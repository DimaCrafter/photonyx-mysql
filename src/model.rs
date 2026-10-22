use std::{borrow::Cow, ffi::c_void, ptr::null};
use bindings::{c::{disown, Slice}, camel_to_kebab, db::{EntityHandle, ModelField, ModelFieldType, ModelMetaImpl}};
use chrono::{DateTime, TimeZone, Utc};
use mysql::{prelude::FromValue, Row};

use crate::{connection::MySqlConnection, schema::Schema};


pub fn fill_model_entity (model: &Box<dyn ModelMetaImpl>, row: Row, entity: EntityHandle) -> Result<(), String> {
	let cols: Vec<Cow<str>> = row.columns_ref().iter().map(|c| c.name_str()).collect();

	for field in model.get_fields() {
		let mut found = false;
		for col in &cols {
			if col == field.name.as_str() {
				found = true;
				break;
			}
		}

		if !found {
			(field.setter)(entity, null());
			continue;
		}

		match field.meta.type_id {
			ModelFieldType::Unknown => {},
			ModelFieldType::U8 => move_value_to_field::<u8>(&row, entity, &field)?,
			ModelFieldType::I8 => move_value_to_field::<i8>(&row, entity, &field)?,
			ModelFieldType::U16 => move_value_to_field::<u16>(&row, entity, &field)?,
			ModelFieldType::I16 => move_value_to_field::<i16>(&row, entity, &field)?,
			ModelFieldType::U32 => move_value_to_field::<u32>(&row, entity, &field)?,
			ModelFieldType::I32 => move_value_to_field::<i32>(&row, entity, &field)?,
			ModelFieldType::F32 => move_value_to_field::<f32>(&row, entity, &field)?,
			ModelFieldType::U64 => move_value_to_field::<u64>(&row, entity, &field)?,
			ModelFieldType::I64 => move_value_to_field::<i64>(&row, entity, &field)?,
			ModelFieldType::F64 => move_value_to_field::<f64>(&row, entity, &field)?,
			ModelFieldType::Bool => move_value_to_field::<bool>(&row, entity, &field)?,
			ModelFieldType::String => move_slice_to_field(&row, entity, &field)?,
			ModelFieldType::Enum => match field.meta.length {
				1 => move_value_to_field::<u8>(&row, entity, &field)?,
				2 => move_value_to_field::<u16>(&row, entity, &field)?,
				4 => move_value_to_field::<u32>(&row, entity, &field)?,
				length => {
					return Err(format!("Field '{}' has invalid type enum(u{})", &field.name, length * 8));
				}
			},
			ModelFieldType::DateTime => parse_value_to_field(&row, parse_datetime, entity, &field)?
		}
	}

	return Ok(());
}

pub fn row_get<T: FromValue> (row: &Row, key: &str) -> Result<T, String> {
	match row.get_opt(key) {
		Some(result) => match result {
			Ok(value) => Ok(value),
			Err(error) => Err(format!("Failed to convert '{key}' value into schema type: {error}"))
		},
		None => Err(format!("Possible field duplication detected: '{key}' was taken twice"))
	}
}

fn move_value_to_field<T: FromValue> (row: &Row, entity: EntityHandle, field: &ModelField) -> Result<(), String> {
	let value: T = row_get(row, &field.name)?;
	(field.setter)(entity, &value as *const T as *const c_void);
	return Ok(());
}

fn parse_value_to_field<R> (row: &Row, parse: fn (mysql::Value) -> Result<R, String>, entity: EntityHandle, field: &ModelField) -> Result<(), String> {
	let value = parse(row_get(row, &field.name)?)?;
	(field.setter)(entity, disown(value) as *const c_void);
	return Ok(());
}

fn parse_datetime (value: mysql::Value) -> Result<DateTime<Utc>, String> {
	if let mysql::Value::Date(year, month, day, hour, min, sec, _) = value {
		let datetime = Utc.with_ymd_and_hms(year as i32, month as u32, day as u32, hour as u32, min as u32, sec as u32);
		if let chrono::offset::LocalResult::Single(parsed) = datetime {
			return Ok(parsed);
		}
	}

	return Err("invalid timestamp received".to_owned());
}

fn move_slice_to_field (row: &Row, entity: EntityHandle, field: &ModelField) -> Result<(), String> {
	let raw: Vec<u8> = row_get(row, &field.name)?;
	if field.meta.length != 0 && raw.len() > field.meta.length {
		return Err(format!("Field '{}' has size {} but database returned {}", &field.name, field.meta.length, raw.len()));
	}

	let slice = Slice::for_vec(&raw);
	(field.setter)(entity, &slice as *const Slice<u8> as *const c_void);

	return Ok(());
}

pub fn get_table_name (collection: &str) -> String {
	let mut table_name = camel_to_kebab(collection);
	if table_name.ends_with('s') {
		table_name.push_str("es");
	} else {
		table_name.push_str("s");
	}

	return table_name;
}

pub fn auto_migrate (db: &mut MySqlConnection, model: &Box<dyn ModelMetaImpl>) -> Result<(), String> {
	let table_name = get_table_name(model.get_name());
	let mut conn = db.get_conn()?;

	let schema = match Schema::fetch_for(&mut conn, &table_name)? {
		Some(schema) => schema,
		None => {
			let schema = Schema::from_model(model)?;
			// println!("Create schema: {:#?}", &schema);

			for fields in model.get_unique_keys() {
				println!("* Unique key {:?}", &fields);
			}

			return Ok(());
		}
	};

	println!("Schema: {:#?}", &schema);
	return Ok(());
}
