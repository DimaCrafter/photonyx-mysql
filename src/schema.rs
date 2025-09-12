use bindings::db::QueryBuilder;
use mysql::{prelude::Queryable, PooledConn, Row};
use crate::{model::eject_value, query::{build_describe_query, SqlPart, SqlQuery}};


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
			src_column: eject_value(&mut row, "COLUMN_NAME")?,
			dst_table: eject_value(&mut row, "REFERENCED_TABLE_NAME")?,
			dst_column: eject_value(&mut row, "REFERENCED_COLUMN_NAME")?,
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
		let name: String = eject_value(&mut row, "Field")?;
		let type_descriptor: String = eject_value(&mut row, "Type")?;
		let key: String = eject_value(&mut row, "Key")?;
		let nullable_raw: String = eject_value(&mut row, "Null")?;
		let extra_raw: String = eject_value(&mut row, "Extra")?;
		let default: Option<String> = eject_value(&mut row, "Default")?;

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
}
