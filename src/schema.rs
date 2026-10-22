use bindings::db::{ModelField, ModelFieldConstraint, ModelFieldType, ModelMetaImpl, QueryBuilder};
use mysql::{prelude::Queryable, PooledConn, Row};
use crate::{model::row_get, query::{build_describe_query, SqlPart, SqlQuery}};


#[derive(Debug)]
pub struct Schema {
	pub fields: Vec<SchemaField>
}

impl Schema {
	pub fn fetch_for (conn: &mut PooledConn, table_name: &str) -> Result<Option<Self>, String> {
		let mut links = KeyLinkRecord::fetch_for(conn, &table_name)?;
		let mut fields = Vec::new();

		let query = build_describe_query(&table_name);
		let iter = match conn.query_iter(&query) {
			Ok(value) => value,
			Err(error) => {
				if let mysql::error::Error::MySqlError(inner) = &error {
					// Table doesn't exist
					if inner.code == 1146 {
						return Ok(None);
					}
				}

				println!("DEBUG: {}", &query);
				return Err(error.to_string());
			}
		};

		for item in iter {
			match item {
				Ok(value) => {
					fields.push(SchemaField::from_row(value, &mut links)?);
				}
				Err(error) => {
					return Err(error.to_string());
				}
			}
		}

		return Ok(Some(Schema { fields }));
	}

	pub fn from_model (model: &Box<dyn ModelMetaImpl>) -> Result<Self, String> {
		let mut schema_fields = Vec::new();
		for field in model.get_fields() {
			match SchemaField::from_model(field) {
				Ok(parsed) => schema_fields.push(parsed),
				Err(error) => {
					return Err(format!("{}: {}", &field.name, error));
				}
			}
		}

		// todo! add unique keys
		return Ok(Schema { fields: schema_fields });
	}
}

#[derive(Debug)]
pub enum SchemaKey {
	None,
	Primary { auto_increment: bool },
	Link { table: String, column: String }
}

pub struct KeyLinkRecord {
	pub src_column: String,
	pub dst_table: String,
	pub dst_column: String
}

impl KeyLinkRecord {
	pub fn from_row (mut row: Row) -> Result<Self, String> {
		Ok(KeyLinkRecord {
			src_column: row_get(&mut row, "COLUMN_NAME")?,
			dst_table: row_get(&mut row, "REFERENCED_TABLE_NAME")?,
			dst_column: row_get(&mut row, "REFERENCED_COLUMN_NAME")?,
		})
	}

	pub fn fetch_for (conn: &mut PooledConn, table_name: &str) -> Result<Vec<Self>, String> {
		let mut link_query_builder = SqlQuery::new("information_schema.key_column_usage");
		link_query_builder.select_by_str(&["COLUMN_NAME", "REFERENCED_TABLE_NAME", "REFERENCED_COLUMN_NAME"]);
		link_query_builder.query(&json::object! {
			"TABLE_SCHEMA": { "$raw": "DATABASE()" },
			"TABLE_NAME": table_name,
			"REFERENCED_TABLE_NAME": { "$neq": null }
		});

		let link_query = link_query_builder.build();
		let link_iter = match conn.exec_iter(&link_query.raw, link_query.params) {
			Ok(value) => value,
			Err(error) => {
				println!("DEBUG: {}", &link_query.raw);
				return Err(error.to_string());
			}
		};

		let mut links = Vec::<KeyLinkRecord>::new();
		for item in link_iter {
			match item {
				Ok(value) => {
					links.push(KeyLinkRecord::from_row(value)?);
				}
				Err(error) => {
					return Err(error.to_string());
				}
			}
		}

		return Ok(links);
	}
}

#[derive(Debug)]
pub struct SchemaField {
	pub name: String,
	pub type_descriptor: String,
	pub key: SchemaKey,
	pub nullable: bool,
	pub default: Option<String>
}

impl SchemaField {
	pub fn from_row (mut row: Row, links: &mut Vec<KeyLinkRecord>) -> Result<Self, String> {
		let name: String = row_get(&mut row, "Field")?;
		let type_descriptor: String = row_get(&mut row, "Type")?;
		let key: String = row_get(&mut row, "Key")?;
		let nullable_raw: String = row_get(&mut row, "Null")?;
		let extra_raw: String = row_get(&mut row, "Extra")?;
		let default: Option<String> = row_get(&mut row, "Default")?;

		let extra: Vec<&str> = extra_raw.split(' ').collect();

		return Ok(SchemaField {
			type_descriptor,
			key: match key.as_str() {
				"PRI" => SchemaKey::Primary { auto_increment: extra.contains(&"auto_increment") },
				"MUL" => {
					match links.iter().position(|link| &link.src_column == &name) {
						Some(link_idx) => {
							let link = links.swap_remove(link_idx);
							SchemaKey::Link {
								table: link.dst_table,
								column: link.dst_column
							}
						},
						None => {
							println!("Supplied 'MUL' key with no link");
							SchemaKey::None
						}
					}
				},
				"" => SchemaKey::None,
				_ => {
					println!("Unknown key type '{}'", &key);
					SchemaKey::None
				}
			},
			name,
			nullable: nullable_raw == "YES",
			default
		});
	}

	pub fn from_model (field: &ModelField) -> Result<Self, String> {
		let mut type_decl = String::new();
		let mut type_attributes = String::new();

		match field.meta.type_id {
			ModelFieldType::DateTime => {
				type_decl.push_str("datetime");
			},
			ModelFieldType::Enum => {
				type_decl.push_str("enum(");

				let mut is_first = true;
				for name in field.meta.iter_enum() {
					if is_first {
						is_first = false;
					} else {
						type_decl.push(',');
					}

					type_decl.push('"');
					type_decl.push_str(name.as_str());
					type_decl.push('"');
				}

				type_decl.push_str(")");
			}
			ModelFieldType::U8 => {
				if field.meta.length != 0 {
					type_decl.push_str("char(");
					type_decl.push_str(&field.meta.length.to_string());
					type_decl.push_str(")");
				} else {
					type_decl.push_str("tinyint");
					type_attributes.push_str(" unsigned");
				}
			},
			ModelFieldType::I8 => {
				if field.meta.length != 0 {
					return Err(format!("i8 array in unsupported"));
				}

				type_decl.push_str("tinyint");
			},
			ModelFieldType::U16 => {
				if field.meta.length != 0 {
					return Err(format!("u16 array in unsupported"));
				}

				type_decl.push_str("smallint");
				type_attributes.push_str(" unsigned");
			},
			ModelFieldType::I16 => {
				if field.meta.length != 0 {
					return Err(format!("i16 array in unsupported"));
				}

				type_decl.push_str("smallint");
			},
			ModelFieldType::F32 => {
				if field.meta.length != 0 {
					return Err(format!("f32 array in unsupported"));
				}

				type_decl.push_str("float");
			},
			ModelFieldType::U32 => {
				if field.meta.length != 0 {
					return Err(format!("u32 array in unsupported"));
				}

				type_decl.push_str("int");
				type_attributes.push_str(" unsigned");
			},
			ModelFieldType::I32 => {
				if field.meta.length != 0 {
					return Err(format!("i32 array in unsupported"));
				}

				type_decl.push_str("int");
			},
			ModelFieldType::F64 => {
				if field.meta.length != 0 {
					return Err(format!("f64 array in unsupported"));
				}

				type_decl.push_str("double");
			},
			ModelFieldType::U64 => {
				if field.meta.length != 0 {
					return Err(format!("u64 array in unsupported"));
				}

				type_decl.push_str("bigint");
				type_attributes.push_str(" unsigned");
			},
			ModelFieldType::I64 => {
				if field.meta.length != 0 {
					return Err(format!("i64 array in unsupported"));
				}

				type_decl.push_str("bigint");
			},
			ModelFieldType::Bool => {
				type_decl.push_str("tinyint(1)");
			},
			ModelFieldType::String => {
				type_decl.push_str("varchar");
			},
			ModelFieldType::Unknown => {
				return Err("unknown type field".to_owned());
			}
		};

		let mut key = SchemaKey::None;
		for constraint in &field.meta.constraints {
			match constraint {
				ModelFieldConstraint::Length { value } => {
					if type_decl.ends_with(')') {
						return Err("unexpected length constraint".to_owned());
					}

					type_decl.push('(');
					type_decl.push_str(&value.to_string());
					type_decl.push(')');
				}
				ModelFieldConstraint::PrimaryKey => {
					key = SchemaKey::Primary { auto_increment: true };
				}
				ModelFieldConstraint::Link { dst_table, dst_column, delete: _, update: _ } => {
					key = SchemaKey::Link { table: dst_table.clone(), column: dst_column.clone() };
					// todo! add {delete:?} on delete, {update:?} on update
				}
			}
		}

		// Only **unsized** `varchar` means `text`
		if type_decl == "varchar" {
			type_decl.clear();
			type_decl.push_str("text");
		}

		type_decl.push_str(&type_attributes);

		return Ok(SchemaField {
			name: field.name.clone(),
			type_descriptor: type_decl,
			key,
			nullable: field.meta.optional,
			default: None // todo! create default value literal
		});
	}
}
