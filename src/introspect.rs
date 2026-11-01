use std::collections::BTreeMap;
use mysql::{prelude::Queryable, Params, Value};
use photonyx_sql_shared::{dialect::{Row, SqlSession, SqlValue}, migrate::SERVICE_TABLE_PREFIX, schema::*};
use crate::dialect::ident;


pub struct MySqlSession<C: Queryable> {
	conn: C
}

impl<C: Queryable> MySqlSession<C> {
	pub fn new (conn: C) -> Self {
		MySqlSession { conn }
	}
}

fn to_mysql (value: &SqlValue) -> Value {
	return match value {
		SqlValue::Null => Value::NULL,
		SqlValue::Int(value) => Value::Int(*value),
		SqlValue::UInt(value) => Value::UInt(*value),
		SqlValue::Float(value) => Value::Double(*value),
		SqlValue::Str(value) => Value::Bytes(value.as_bytes().to_vec()),
		SqlValue::Bytes(value) => Value::Bytes(value.clone())
	};
}

fn from_mysql (value: Value) -> SqlValue {
	return match value {
		Value::NULL => SqlValue::Null,
		Value::Bytes(bytes) => SqlValue::Bytes(bytes),
		Value::Int(value) => SqlValue::Int(value),
		Value::UInt(value) => SqlValue::UInt(value),
		Value::Float(value) => SqlValue::Float(value as f64),
		Value::Double(value) => SqlValue::Float(value),
		Value::Date(year, month, day, hour, minute, second, _) => {
			SqlValue::Str(format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"))
		}
		Value::Time(negative, days, hours, minutes, seconds, _) => {
			SqlValue::Str(format!("{}{:02}:{minutes:02}:{seconds:02}", if negative { "-" } else { "" }, days * 24 + hours as u32))
		}
	};
}

impl<C: Queryable> SqlSession for MySqlSession<C> {
	fn query (&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Row>, String> {
		let rows_q = if params.is_empty() {
			self.conn.query(sql)
		} else {
			self.conn.exec(sql, Params::Positional(params.iter().map(to_mysql).collect()))
		};
		
		let rows: Vec<mysql::Row> = rows_q.map_err(|error| error.to_string())?;

		return Ok(rows.into_iter().map(|row| row.unwrap().into_iter().map(from_mysql).collect()).collect());
	}

	fn exec (&mut self, sql: &str) -> Result<(), String> {
		return self.conn.query_drop(sql).map_err(|error| error.to_string());
	}
}


/// Text of the `index`-th value of a row, `None` for NULL
fn cell (row: &Row, index: usize) -> Option<String> {
	return row.get(index).and_then(SqlValue::text);
}

fn text_of (row: &Row, index: usize) -> String {
	return cell(row, index).unwrap_or_default();
}

pub fn is_mariadb (session: &mut dyn SqlSession) -> Result<bool, String> {
	let rows = session.query("SELECT VERSION()", &[])?;
	return Ok(rows.first().and_then(|row| cell(row, 0)).is_some_and(|version| version.contains("MariaDB")));
}

pub fn table_exists (session: &mut dyn SqlSession, table: &str) -> Result<bool, String> {
	let rows = session.query(
		"SELECT 1 FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?",
		&[SqlValue::Str(table.to_owned())]
	)?;
	return Ok(!rows.is_empty());
}

pub fn lock (session: &mut dyn SqlSession) -> Result<(), String> {
	const TIMEOUT: u32 = 30;
	let rows = session.query(
		&format!("SELECT GET_LOCK(CONCAT('photonyx_migrations.', DATABASE()), {TIMEOUT})"), 
		&[]
	)?;
	
	return match rows.first().and_then(|row| cell(row, 0)).as_deref() {
		Some("1") => Ok(()),
		Some("0") => Err(format!("another `onyx migrate` holds the migrations lock of this database (waited {TIMEOUT} s)")),
		_ => Err("GET_LOCK failed".to_owned())
	};
}


pub fn introspect (session: &mut dyn SqlSession) -> Result<DbSchema, String> {
	let mariadb = is_mariadb(session)?;
	let mut schema = DbSchema::default();

	let rows = session.query(
		"SELECT t.TABLE_NAME, t.ENGINE, t.TABLE_COLLATION, c.CHARACTER_SET_NAME \
		FROM information_schema.TABLES t \
		LEFT JOIN information_schema.COLLATION_CHARACTER_SET_APPLICABILITY c ON c.COLLATION_NAME = t.TABLE_COLLATION \
		WHERE t.TABLE_SCHEMA = DATABASE() AND t.TABLE_TYPE = 'BASE TABLE'",
		&[]
	)?;

	for row in &rows {
		let name = text_of(row, 0);
		if name.starts_with(SERVICE_TABLE_PREFIX) {
			continue;
		}

		let mut table = TableSchema::new(&name);
		for (key, index) in [("engine", 1), ("collation", 2), ("charset", 3)] {
			if let Some(value) = cell(row, index) {
				table.options.insert(key.to_owned(), value);
			}
		}

		schema.tables.insert(name, table);
	}

	// MariaDB keeps JSON as longtext + CHECK (json_valid(`col`)) named after the column
	let mut json_checks: BTreeMap<(String, String), ()> = BTreeMap::new();
	if mariadb {
		let rows = session.query("SELECT TABLE_NAME, CONSTRAINT_NAME, CHECK_CLAUSE FROM information_schema.CHECK_CONSTRAINTS WHERE CONSTRAINT_SCHEMA = DATABASE()", &[])?;
		for row in &rows {
			let (table, name, clause) = (text_of(row, 0), text_of(row, 1), text_of(row, 2));
			if clause.replace(' ', "").eq_ignore_ascii_case(&format!("json_valid({})", ident(&name))) {
				json_checks.insert((table, name), ());
			}
		}
	}

	let rows = session.query(
		"SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, EXTRA, COLLATION_NAME, GENERATION_EXPRESSION FROM information_schema.COLUMNS \
		WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME, ORDINAL_POSITION",
		&[]
	)?;

	for row in &rows {
		let table = match schema.tables.get_mut(&text_of(row, 0)) {
			Some(table) => table,
			None => continue
		};

		let name = text_of(row, 1);
		let raw_type = text_of(row, 2);
		let nullable = text_of(row, 3) == "YES";
		let extra = text_of(row, 5).to_ascii_lowercase();

		let json = json_checks.contains_key(&(table.name.clone(), name.clone())) && raw_type.eq_ignore_ascii_case("longtext");
		let db_type = if json { ColumnType::Json } else { column_type(&raw_type) };

		let generated = if extra.contains("virtual generated") || extra.contains("stored generated") {
			cell(row, 7)
				.filter(|expr| !expr.is_empty())
				.map(|expr| Generated { expr, stored: extra.contains("stored generated") })
		} else {
			None
		};

		let default = if generated.is_some() { None } else { column_default(cell(row, 4).as_deref(), &extra, &db_type, mariadb) };
		let on_update_now = extra.contains("on update current_timestamp");
		
		let collation = cell(row, 6)
			.filter(|collation| !matches!(db_type, ColumnType::Json) && Some(collation) != table.options.get("collation"));

		table.columns.push(ColumnSchema {
			name,
			db_type,
			nullable,
			default,
			on_update_now,
			generated,
			auto_increment: extra.split_whitespace().any(|word| word == "auto_increment"),
			collation
		});
	}

	let names: Vec<String> = schema.tables.keys().cloned().collect();
	for name in &names {
		let rows = session.query(&format!("SHOW INDEX FROM {}", ident(name)), &[])?;
		let table = schema.tables.get_mut(name).unwrap();

		for row in &rows {
			// Table, Non_unique, Key_name, Seq_in_index, Column_name, Collation, Cardinality, Sub_part, Packed, Null, Index_type
			let key = text_of(row, 2);
			let column = match (cell(row, 4), cell(row, 7)) {
				(Some(column), Some(part)) => format!("{column}({part})"),
				(Some(column), None) => column,
				// Functional index part on MySQL 8
				(None, _) => "(expression)".to_owned()
			};

			if key == "PRIMARY" {
				table.primary.push(column);
				continue;
			}

			match table.indexes.iter_mut().find(|index| index.name == key) {
				Some(index) => index.columns.push(column),
				None => {
					let using = text_of(row, 10).to_ascii_lowercase();
					table.indexes.push(IndexSchema {
						name: key,
						columns: vec![column],
						unique: text_of(row, 1) == "0",
						using: if using == "btree" || using.is_empty() { None } else { Some(using) }
					});
				}
			}
		}
	}

	let rows = session.query(
		"SELECT k.TABLE_NAME, k.CONSTRAINT_NAME, k.COLUMN_NAME, k.REFERENCED_TABLE_NAME, k.REFERENCED_COLUMN_NAME, r.UNIQUE_CONSTRAINT_NAME, r.DELETE_RULE \
		FROM information_schema.KEY_COLUMN_USAGE k JOIN information_schema.REFERENTIAL_CONSTRAINTS r \
		ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA AND r.TABLE_NAME = k.TABLE_NAME AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME \
		WHERE k.TABLE_SCHEMA = DATABASE() AND k.REFERENCED_TABLE_NAME IS NOT NULL \
		ORDER BY k.TABLE_NAME, k.CONSTRAINT_NAME, k.ORDINAL_POSITION",
		&[]
	)?;

	for row in &rows {
		let table = match schema.tables.get_mut(&text_of(row, 0)) {
			Some(table) => table,
			None => continue
		};

		let name = text_of(row, 1);
		let position = match table.foreign_keys.iter().position(|fk| fk.name == name) {
			Some(position) => position,
			None => {
				let ref_index = cell(row, 5).filter(|index| index != "PRIMARY");
				table.foreign_keys.push(ForeignKey {
					index: String::new(),
					columns: Vec::new(),
					ref_table: text_of(row, 3),
					ref_columns: Vec::new(),
					ref_index,
					on_delete: match text_of(row, 6).as_str() {
						"CASCADE" => OnDelete::Cascade,
						"SET NULL" => OnDelete::SetNull,
						_ => OnDelete::Restrict
					},
					name
				});
				table.foreign_keys.len() - 1
			}
		};

		let fk = &mut table.foreign_keys[position];
		fk.columns.push(text_of(row, 2));
		fk.ref_columns.push(text_of(row, 4));
	}

	for table in schema.tables.values_mut() {
		let indexes = table.indexes.clone();
		let primary = table.primary.clone();
		for fk in table.foreign_keys.iter_mut() {
			fk.index = fk_index(fk, &indexes, &primary);
		}
	}

	return Ok(schema);
}

fn fk_index (fk: &ForeignKey, indexes: &[IndexSchema], primary: &[String]) -> String {
	let exact = indexes.iter().filter(|index| index.columns == fk.columns);
	if let Some(index) = exact.clone().find(|index| index.name == fk.name).or_else(|| exact.clone().next()) {
		return index.name.clone();
	}

	if let Some(index) = indexes.iter().find(|index| index.columns.starts_with(&fk.columns)) {
		return index.name.clone();
	}

	if primary.starts_with(&fk.columns) {
		return "PRIMARY".to_owned();
	}

	return fk.name.clone();
}

pub fn column_type (raw: &str) -> ColumnType {
	let lower = raw.trim().to_ascii_lowercase();
	let other = || ColumnType::Other(lower.clone());

	if lower.starts_with("enum(") && lower.ends_with(')') {
		return match enum_values(&raw.trim()[5..raw.trim().len() - 1]) {
			Some(values) => ColumnType::Enum(values),
			None => other()
		};
	}

	let (base, unsigned) = match lower.strip_suffix(" unsigned") {
		Some(base) => (base, true),
		None => (lower.as_str(), false)
	};

	let (name, args) = match base.split_once('(') {
		Some((name, rest)) => match rest.strip_suffix(')') {
			Some(args) => (name, Some(args)),
			None => return other()
		},
		None => (base, None)
	};

	let size = || args.and_then(|args| args.parse::<u32>().ok());
	let int = |size: IntSize| ColumnType::Int { size, unsigned };

	return match name {
		"tinyint" if args == Some("1") && !unsigned => ColumnType::Bool,
		"tinyint" => int(IntSize::Tiny),
		"smallint" => int(IntSize::Small),
		"mediumint" => int(IntSize::Medium),
		"int" | "integer" => int(IntSize::Normal),
		"bigint" => int(IntSize::Big),
		"float" if args.is_none() && !unsigned => ColumnType::Float,
		"double" if args.is_none() && !unsigned => ColumnType::Double,
		"varchar" => size().map(ColumnType::Varchar).unwrap_or_else(other),
		"char" => size().map(ColumnType::Char).unwrap_or_else(other),
		"text" if args.is_none() => ColumnType::Text,
		"datetime" if args.is_none() => ColumnType::DateTime,
		"json" => ColumnType::Json,
		_ => other()
	};
}

/// `'a','b''c'` --> `[a, b'c]`
fn enum_values (raw: &str) -> Option<Vec<String>> {
	let mut result = Vec::new();
	let mut rest = raw.trim();
	
	while !rest.is_empty() {
		let (value, tail) = string_literal(rest)?;
		result.push(value);
		rest = tail.trim_start();
		rest = match rest.strip_prefix(',') {
			Some(tail) => tail.trim_start(),
			None if rest.is_empty() => rest,
			None => return None
		};
	}

	return Some(result);
}

pub fn string_literal (raw: &str) -> Option<(String, &str)> {
	let mut chars = raw.char_indices();
	if chars.next()?.1 != '\'' {
		return None;
	}

	let mut value = String::new();
	while let Some((index, ch)) = chars.next() {
		match ch {
			'\'' => {
				if raw[index + 1..].starts_with('\'') {
					chars.next();
					value.push('\'');
				} else {
					return Some((value, &raw[index + 1..]));
				}
			}
			'\\' => {
				let (_, escaped) = chars.next()?;
				value.push(match escaped {
					'n' => '\n',
					'r' => '\r',
					't' => '\t',
					'0' => '\0',
					'Z' => '\x1A',
					other => other
				});
			}
			other => value.push(other)
		}
	}

	return None;
}

pub fn column_default (raw: Option<&str>, extra: &str, ty: &ColumnType, mariadb: bool) -> Option<DefaultValue> {
	let raw = raw?;

	let value = if mariadb {
		if raw == "NULL" {
			return None;
		}

		match string_literal(raw) {
			Some((value, rest)) if rest.is_empty() => Literal::Str(value),
			_ => Literal::Expr(raw.to_owned())
		}
	} else if extra.contains("default_generated") {
		Literal::Expr(raw.to_owned())
	} else {
		Literal::Str(raw.to_owned())
	};

	let value = match value {
		Literal::Expr(expr) => {
			let lower = expr.to_ascii_lowercase();
			if lower == "current_timestamp" || lower == "current_timestamp()" || lower == "now()" {
				return Some(DefaultValue::Now);
			}

			// MySQL expression default: `_utf8mb4\'it\\\'s\'` is `_utf8mb4'it\'s'` with its quotes escaped
			match mysql_expression_literal(&expr) {
				Some(value) => value,
				// MariaDB keeps numbers without quotes; anything else is an expression (`uuid()`)
				None if expr.parse::<f64>().is_ok() => expr,
				None => return Some(DefaultValue::Expr(expr))
			}
		}
		Literal::Str(value) => value
	};

	return Some(match ty {
		ColumnType::Int { .. } => value.parse::<i64>().map(DefaultValue::Int).unwrap_or(DefaultValue::Str(value)),
		ColumnType::Float | ColumnType::Double => value.parse::<f64>().map(DefaultValue::Float).unwrap_or(DefaultValue::Str(value)),
		ColumnType::Bool => match value.as_str() {
			"0" => DefaultValue::Bool(false),
			"1" => DefaultValue::Bool(true),
			_ => DefaultValue::Str(value)
		},
		_ => DefaultValue::Str(value)
	});
}

enum Literal {
	Str(String),
	Expr(String)
}

/// `_utf8mb4\'text\'` / `\'text\'` / `('text')` --> `text`
fn mysql_expression_literal (expr: &str) -> Option<String> {
	let mut unescaped = String::new();
	let mut chars = expr.chars();
	while let Some(ch) = chars.next() {
		if ch == '\\' {
			unescaped.push(chars.next()?);
		} else {
			unescaped.push(ch);
		}
	}

	let mut rest = unescaped.trim();
	while let Some(inner) = rest.strip_prefix('(').and_then(|inner| inner.strip_suffix(')')) {
		rest = inner.trim();
	}

	// Charset introducer
	if let Some(position) = rest.find('\'') {
		let introducer = &rest[..position];
		if !(introducer.is_empty() || introducer.starts_with('_') && introducer[1..].chars().all(|ch| ch.is_ascii_alphanumeric())) {
			return None;
		}

		rest = &rest[position..];
	}

	return match string_literal(rest) {
		Some((value, tail)) if tail.trim().is_empty() => Some(value),
		_ => None
	};
}
