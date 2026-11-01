use std::collections::BTreeMap;
use json::JsonValue;
use photonyx_models::ir::{DialectBlock, Enum, Field, FieldType, Index, Raw, Table};
use mysql::Conn;
use photonyx_sql_shared::{dialect::{ColumnOptions, SqlSession}, migrate, schema::*, DbSchema, Op, SqlDialect, Statement};
use crate::{connection::connection_opts, introspect::{self, MySqlSession}};


pub struct MySqlDialect;

/// Longest `varchar` that always fits into a row with utf8mb4 (65535 bytes / 4)
const MAX_VARCHAR: u32 = 16383;
const MAX_CHAR: u32 = 255;
const DEFAULT_CHARSET: &str = "utf8mb4";
const DEFAULT_COLLATION: &str = "utf8mb4_unicode_ci";

impl SqlDialect for MySqlDialect {
	fn id (&self) -> &str {
		return "mysql";
	}

	fn map_type (&self, field: &Field, enum_entity: Option<&Enum>) -> Result<ColumnType, String> {
		let int = |size: IntSize, unsigned: bool| ColumnType::Int { size, unsigned };

		return Ok(match field.field_type {
			FieldType::U8 => int(IntSize::Tiny, true),
			FieldType::U16 => int(IntSize::Small, true),
			FieldType::U32 => int(IntSize::Normal, true),
			FieldType::U64 => int(IntSize::Big, true),
			FieldType::I8 => int(IntSize::Tiny, false),
			FieldType::I16 => int(IntSize::Small, false),
			FieldType::I32 => int(IntSize::Normal, false),
			FieldType::I64 => int(IntSize::Big, false),
			FieldType::F32 => ColumnType::Float,
			FieldType::F64 => ColumnType::Double,
			FieldType::Bool => ColumnType::Bool,
			FieldType::String(size) => {
				if size > MAX_VARCHAR {
					return Err(format!("string({size}) is longer than {MAX_VARCHAR} (varchar with utf8mb4), use text"));
				}

				ColumnType::Varchar(size)
			}
			FieldType::Char(size) => {
				if size > MAX_CHAR {
					return Err(format!("char({size}) is longer than {MAX_CHAR}, use string(N)"));
				}

				ColumnType::Char(size)
			}
			FieldType::Text => ColumnType::Text,
			FieldType::DateTime => ColumnType::DateTime,
			FieldType::Json => ColumnType::Json,
			FieldType::Enum => {
				let entity = enum_entity.ok_or("type `enum` requires `enum: <Entity>`")?;
				ColumnType::Enum(entity.values.iter().map(|value| value.db_name().to_owned()).collect())
			}
		});
	}

	fn column_options (&self, field: &Field) -> Result<ColumnOptions, String> {
		let mut strings = block_values(&field.dialects, &["collation", "default", "generated", "stored"])?;
		let text = |value: Raw, key: &str| match value {
			Raw::Str(value) if !value.trim().is_empty() => Ok(value),
			_ => Err(format!("mysql.{key}: expected a non-empty string"))
		};

		let collation = strings.remove("collation").map(|value| text(value, "collation")).transpose()?;
		let default = strings.remove("default").map(|value| text(value, "default")).transpose()?;
		let stored = match strings.remove("stored") {
			None => None,
			Some(Raw::Bool(value)) => Some(value),
			Some(_) => return Err("mysql.stored: expected true or false".to_owned())
		};

		let generated = match strings.remove("generated") {
			Some(value) => {
				if !field.nullable {
					return Err("mysql.generated: a generated column must be nullable (`?T`)".to_owned());
				}

				Some(Generated { expr: text(value, "generated")?, stored: stored.unwrap_or(false) })
			}
			None if stored.is_some() => return Err("mysql.stored: only together with mysql.generated".to_owned()),
			None => None
		};

		return Ok(ColumnOptions { collation, default, generated });
	}

	fn index_using (&self, index: &Index) -> Result<Option<String>, String> {
		let mut options = block_strings(&index.dialects, &["using"])?;
		return match options.remove("using") {
			Some(using) => match using.to_ascii_lowercase().as_str() {
				"btree" | "hash" => Ok(Some(using.to_ascii_lowercase())),
				_ => Err(format!("mysql.using: expected btree or hash, got `{using}`"))
			},
			None => Ok(None)
		};
	}

	fn table_options (&self, table: &Table, defaults: &[DialectBlock]) -> Result<BTreeMap<String, String>, String> {
		const KEYS: &[&str] = &["engine", "charset", "collation"];

		let mut result = block_strings(defaults, KEYS).map_err(|error| format!("config: {error}"))?;
		result.extend(block_strings(&table.dialects, KEYS)?);

		if !result.contains_key("charset") {
			result.insert("charset".to_owned(), DEFAULT_CHARSET.to_owned());
		}

		if !result.contains_key("collation") && result["charset"] == DEFAULT_CHARSET {
			result.insert("collation".to_owned(), DEFAULT_COLLATION.to_owned());
		}

		return Ok(result);
	}

	fn render (&self, ops: &[Op]) -> Vec<Statement> {
		let mut result = Vec::new();
		for (index, op) in ops.iter().enumerate() {
			if let Some(sql) = render_op(op) {
				result.push(Statement { sql, op_idx: index });
			}
		}

		return result;
	}

	fn transactional_ddl (&self) -> bool {
		return false;
	}

	fn connect (&self, raw_config: &JsonValue) -> Result<Box<dyn SqlSession>, String> {
		let conn = Conn::new(connection_opts(raw_config)?).map_err(|error| error.to_string())?;
		return Ok(Box::new(MySqlSession::new(conn)));
	}

	fn introspect (&self, session: &mut dyn SqlSession) -> Result<DbSchema, String> {
		return introspect::introspect(session);
	}

	fn table_exists (&self, session: &mut dyn SqlSession, table: &str) -> Result<bool, String> {
		return introspect::table_exists(session, table);
	}

	fn import_table_options (&self, options: &BTreeMap<String, String>, defaults: &[DialectBlock]) -> Result<BTreeMap<String, String>, String> {
		let empty = Table { table: String::new(), fields: Vec::new(), indexes: Vec::new(), deleted: false, dialects: Vec::new() };
		let base = self.table_options(&empty, defaults)?;

		let mut result = BTreeMap::new();
		for (key, value) in options {
			let same = base.get(key).is_some_and(|wanted| wanted.eq_ignore_ascii_case(value));
			// InnoDB is the default engine of both servers
			let default_engine = key == "engine" && !base.contains_key("engine") && value.eq_ignore_ascii_case("InnoDB");
			if !same && !default_engine {
				result.insert(key.clone(), value.clone());
			}
		}

		return Ok(result);
	}

	fn lock (&self, session: &mut dyn SqlSession) -> Result<(), String> {
		return introspect::lock(session);
	}

	/// `-- ` and `#` to the end of the line, `/* */`
	fn comment_end (&self, chars: &[char], at: usize) -> Option<usize> {
		let ch = chars[at];
		let next = chars.get(at + 1).copied();
		let line_comment = ch == '-' && next == Some('-') && chars.get(at + 2).is_none_or(|ch| ch.is_whitespace())
			|| ch == '#' && (at == 0 || chars[at - 1].is_whitespace() || chars[at - 1] == ';');

		if line_comment {
			return Some(migrate::line_end(chars, at));
		}

		if ch == '/' && next == Some('*') {
			return Some(migrate::block_end(chars, at));
		}

		return None;
	}

	/// Backslash escapes in '...' and "...", backticks quote identifiers
	fn quoted_end (&self, chars: &[char], at: usize, _before: &str) -> Option<usize> {
		let ch = chars[at];
		if ch == '\'' || ch == '"' || ch == '`' {
			return Some(migrate::quote_end(chars, at, ch != '`'));
		}

		return None;
	}
}

fn block_values (blocks: &[DialectBlock], allowed: &[&str]) -> Result<BTreeMap<String, Raw>, String> {
	let mut result = BTreeMap::new();
	for block in blocks.iter().filter(|block| block.dialect == "mysql") {
		let entries = match &block.value {
			Raw::Map(entries) => entries,
			_ => return Err("mysql: expected an object".to_owned())
		};

		for (key, value) in entries {
			if !allowed.contains(&key.as_str()) {
				return Err(format!("mysql: unknown key `{key}` (expected {})", allowed.join(", ")));
			}

			result.insert(key.clone(), value.clone());
		}
	}

	return Ok(result);
}

fn block_strings (blocks: &[DialectBlock], allowed: &[&str]) -> Result<BTreeMap<String, String>, String> {
	let mut result = BTreeMap::new();
	for block in blocks.iter().filter(|block| block.dialect == "mysql") {
		let entries = match &block.value {
			Raw::Map(entries) => entries,
			_ => return Err("mysql: expected an object".to_owned())
		};

		for (key, value) in entries {
			if !allowed.contains(&key.as_str()) {
				return Err(format!("mysql: unknown key `{key}` (expected {})", allowed.join(", ")));
			}

			match value {
				Raw::Str(value) => result.insert(key.clone(), value.clone()),
				_ => return Err(format!("mysql.{key}: expected a string"))
			};
		}
	}

	return Ok(result);
}


pub fn ident (name: &str) -> String {
	return format!("`{}`", name.replace('`', "``"));
}

fn ident_list (names: &[String]) -> String {
	return names.iter().map(|name| ident(name)).collect::<Vec<_>>().join(", ");
}

pub fn string_literal (value: &str) -> String {
	let mut result = String::from("'");
	for ch in value.chars() {
		match ch {
			'\'' => result.push_str("''"),
			'\\' => result.push_str("\\\\"),
			'\n' => result.push_str("\\n"),
			'\r' => result.push_str("\\r"),
			'\0' => result.push_str("\\0"),
			_ => result.push(ch)
		}
	}

	result.push('\'');
	return result;
}

pub fn type_sql (ty: &ColumnType) -> String {
	return match ty {
		ColumnType::Int { size, unsigned } => {
			let name = match size {
				IntSize::Tiny => "tinyint",
				IntSize::Small => "smallint",
				IntSize::Medium => "mediumint",
				IntSize::Normal => "int",
				IntSize::Big => "bigint"
			};

			if *unsigned { format!("{name} unsigned") } else { name.to_owned() }
		}
		ColumnType::Float => "float".to_owned(),
		ColumnType::Double => "double".to_owned(),
		ColumnType::Bool => "tinyint(1)".to_owned(),
		ColumnType::Varchar(size) => format!("varchar({size})"),
		ColumnType::Char(size) => format!("char({size})"),
		ColumnType::Text => "text".to_owned(),
		ColumnType::DateTime => "datetime".to_owned(),
		ColumnType::Json => "json".to_owned(),
		ColumnType::Enum(values) => {
			format!("enum({})", values.iter().map(|value| string_literal(value)).collect::<Vec<_>>().join(","))
		},
		ColumnType::EnumType(name) => ident(name),
		ColumnType::Other(raw) => raw.clone()
	};
}

fn default_sql (column: &ColumnSchema, value: &DefaultValue) -> String {
	return match value {
		DefaultValue::Int(value) => value.to_string(),
		DefaultValue::Float(value) => value.to_string(),
		DefaultValue::Bool(value) => if *value { "1" } else { "0" }.to_owned(),
		DefaultValue::Now => "CURRENT_TIMESTAMP".to_owned(),
		// MySQL 8 takes expression defaults only in parentheses, MariaDB takes both
		DefaultValue::Expr(expr) => format!("({})", strip_parens(expr)),
		DefaultValue::Str(value) => {
			// MySQL 8 allows only expression defaults for text/json, MariaDB accepts both
			if matches!(column.db_type, ColumnType::Text | ColumnType::Json) {
				format!("({})", string_literal(value))
			} else {
				string_literal(value)
			}
		}
	};
}

/// `(expr)` --> `expr`, when the parentheses enclose the whole expression
fn strip_parens (expr: &str) -> &str {
	let trimmed = expr.trim();
	if let Some(inner) = trimmed.strip_prefix('(').and_then(|inner| inner.strip_suffix(')')) {
		let mut depth = 0i32;
		let mut quoted = false;
		let encloses = inner.chars().all(|ch| {
			match ch {
				'\'' => quoted = !quoted,
				'(' if !quoted => depth += 1,
				')' if !quoted => depth -= 1,
				_ => {}
			}
			depth >= 0
		});

		if encloses && depth == 0 {
			return strip_parens(inner);
		}
	}

	return trimmed;
}

pub fn column_sql (column: &ColumnSchema) -> String {
	let mut result = format!("{} {}", ident(&column.name), type_sql(&column.db_type));
	if let Some(collation) = &column.collation {
		result.push_str(" COLLATE ");
		result.push_str(collation);
	}

	if let Some(generated) = &column.generated {
		result.push_str(
			&format!(
				" GENERATED ALWAYS AS ({}) {}",
				strip_parens(&generated.expr),
				if generated.stored { "STORED" } else { "VIRTUAL" }
			)
		);
	} else {
		result.push_str(if column.nullable { " NULL" } else { " NOT NULL" });
	}
	if let Some(value) = &column.default {
		result.push_str(" DEFAULT ");
		result.push_str(&default_sql(column, value));
	}

	if column.on_update_now {
		result.push_str(" ON UPDATE CURRENT_TIMESTAMP");
	}

	if column.auto_increment {
		result.push_str(" AUTO_INCREMENT");
	}

	return result;
}

fn index_sql (index: &IndexSchema) -> String {
	let mut result = format!(
		"{}KEY {} ({})",
		if index.unique { "UNIQUE " } else { "" },
		ident(&index.name),
		ident_list(&index.columns)
	);

	if let Some(using) = &index.using {
		result.push_str(" USING ");
		result.push_str(&using.to_ascii_uppercase());
	}

	return result;
}

fn foreign_key_sql (fk: &ForeignKey) -> String {
	let on_delete = match fk.on_delete {
		OnDelete::Restrict => "RESTRICT",
		OnDelete::Cascade => "CASCADE",
		OnDelete::SetNull => "SET NULL",
		OnDelete::NoAction => "NO ACTION"
	};

	return format!(
		"CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({}) ON DELETE {on_delete}",
		ident(&fk.name), ident_list(&fk.columns), ident(&fk.ref_table), ident_list(&fk.ref_columns)
	);
}

fn options_sql (options: &BTreeMap<String, String>) -> Vec<String> {
	let mut result = Vec::new();
	if let Some(engine) = options.get("engine") {
		result.push(format!("ENGINE={engine}"));
	}

	if let Some(charset) = options.get("charset") {
		result.push(format!("DEFAULT CHARSET={charset}"));
	}

	if let Some(collation) = options.get("collation") {
		result.push(format!("COLLATE={collation}"));
	}

	return result;
}

fn render_op (op: &Op) -> Option<String> {
	let alter = |table: &str, action: String| format!("ALTER TABLE {} {action}", ident(table));

	return Some(match op {
		Op::CreateTable(table) => {
			let mut lines: Vec<String> = table.columns.iter().map(column_sql).collect();
			if !table.primary.is_empty() {
				lines.push(format!("PRIMARY KEY ({})", ident_list(&table.primary)));
			}

			lines.extend(table.indexes.iter().map(index_sql));
			lines.extend(table.foreign_keys.iter().map(foreign_key_sql));

			let mut sql = format!("CREATE TABLE {} (\n  {}\n)", ident(&table.name), lines.join(",\n  "));
			for option in options_sql(&table.options) {
				sql.push(' ');
				sql.push_str(&option);
			}

			sql
		}
		Op::DropTable { table } => format!("DROP TABLE {}", ident(table)),
		Op::AddColumn { table, column, after } => {
			let position = match after {
				Some(previous) => format!(" AFTER {}", ident(previous)),
				None => " FIRST".to_owned()
			};

			alter(table, format!("ADD COLUMN {}{position}", column_sql(column)))
		}
		Op::DropColumn { table, column } => alter(table, format!("DROP COLUMN {}", ident(column))),
		Op::ModifyColumn { table, to, .. } => alter(table, format!("MODIFY COLUMN {}", column_sql(to))),
		Op::AddPrimaryKey { table, columns } => alter(table, format!("ADD PRIMARY KEY ({})", ident_list(columns))),
		Op::DropPrimaryKey { table } => alter(table, "DROP PRIMARY KEY".to_owned()),
		Op::AddIndex { table, index } => alter(table, format!("ADD {}", index_sql(index))),
		Op::DropIndex { table, index } => alter(table, format!("DROP INDEX {}", ident(index))),
		Op::AddForeignKey { table, fk } => alter(table, format!("ADD {}", foreign_key_sql(fk))),
		Op::DropForeignKey { table, name } => alter(table, format!("DROP FOREIGN KEY {}", ident(name))),
		Op::AlterTableOptions { table, from, to } => {
			let mut changed: BTreeMap<String, String> = to
				.iter()
				.filter(|(key, value)| from.get(*key) != Some(*value))
				.map(|(key, value)| (key.clone(), value.clone()))
				.collect();

			if changed.contains_key("charset") || changed.contains_key("collation") {
				for key in ["charset", "collation"] {
					if let Some(value) = to.get(key) {
						changed.insert(key.to_owned(), value.clone());
					}
				}
			}

			let parts = options_sql(&changed);
			if parts.is_empty() {
				return None;
			}

			alter(table, parts.join(", "))
		}

		Op::SetUpdateNow { .. }
		| Op::CreateUpdateNowFunction
		| Op::DropUpdateNowFunction
		| Op::CreateEnumType { .. }
		| Op::AddEnumValues { .. }
		| Op::RecreateEnumType { .. }
		| Op::DropEnumType { .. } => return None
	});
}
