use std::{ffi::c_void, ptr::null};
use bindings::{c::Slice, camel_to_kebab, db::{EntityHandle, ModelField, ModelFieldType, ModelMetaImpl, ModelFieldConstraint}};
use mysql::{prelude::{FromValue, Queryable}, Row};

use crate::{connection::MySqlConnection, query::{build_describe_query, SqlPart, SqlQuery}, schema::{KeyLinkRecord, Schema, SchemaField}};


pub fn fill_model_entity (model: &Box<dyn ModelMetaImpl>, mut row: Row, entity: EntityHandle) -> Result<(), String> {
	//? copy to avoid &mut while holding &const
	let cols: Vec<String> = row.columns_ref().iter().map(|c| c.name_str().into_owned()).collect();

	for field in model.get_fields() {
		if !cols.contains(&field.name) {
			(field.setter)(entity, null());
			continue;
		}

		match field.meta.type_id {
			ModelFieldType::Unknown => {},
			ModelFieldType::U8 => move_value_to_field::<u8>(&mut row, entity, &field)?,
			ModelFieldType::I8 => move_value_to_field::<i8>(&mut row, entity, &field)?,
			ModelFieldType::U16 => move_value_to_field::<u16>(&mut row, entity, &field)?,
			ModelFieldType::I16 => move_value_to_field::<i16>(&mut row, entity, &field)?,
			ModelFieldType::U32 => move_value_to_field::<u32>(&mut row, entity, &field)?,
			ModelFieldType::I32 => move_value_to_field::<i32>(&mut row, entity, &field)?,
			ModelFieldType::F32 => move_value_to_field::<f32>(&mut row, entity, &field)?,
			ModelFieldType::U64 => move_value_to_field::<u64>(&mut row, entity, &field)?,
			ModelFieldType::I64 => move_value_to_field::<i64>(&mut row, entity, &field)?,
			ModelFieldType::F64 => move_value_to_field::<f64>(&mut row, entity, &field)?,
			ModelFieldType::Bool => move_value_to_field::<bool>(&mut row, entity, &field)?,
			ModelFieldType::String => move_slice_to_field(&mut row, entity, &field)?,
			ModelFieldType::Array => move_slice_to_field(&mut row, entity, &field)?,
			ModelFieldType::Enum => match field.meta.length {
				1 => move_value_to_field::<u8>(&mut row, entity, &field)?,
				2 => move_value_to_field::<u16>(&mut row, entity, &field)?,
				4 => move_value_to_field::<u32>(&mut row, entity, &field)?,
				length => {
					return Err(format!("Field '{}' has invalid type enum(u{})", &field.name, length * 8));
				}
			}
		}
	}

	return Ok(());
}

pub fn eject_value<T: FromValue> (row: &mut Row, key: &str) -> Result<T, String> {
	match row.take_opt(key) {
		Some(result) => match result {
			Ok(value) => Ok(value),
			Err(error) => Err(format!("Failed to convert '{key}' value into schema type: {error}"))
		},
		None => Err(format!("Possible field duplication detected: '{key}' was taken twice"))
	}
}

fn move_value_to_field<T: FromValue> (row: &mut Row, entity: EntityHandle, field: &ModelField) -> Result<(), String> {
	let value: T = eject_value(row, &field.name)?;
	(field.setter)(entity, &value as *const T as *const c_void);
	return Ok(());
}

fn move_slice_to_field (row: &mut Row, entity: EntityHandle, field: &ModelField) -> Result<(), String> {
	let raw: Vec<u8> = eject_value(row, &field.name)?;
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
			for field in model.get_fields() {
				match field.meta.type_id {
					ModelFieldType::Enum => {
						println!("{}: enum (u{})", field.name, field.meta.length * 8);
						for i in 0..field.meta.enum_size {
							let ptr = unsafe { field.meta.enum_values.offset(i as isize) };
							let name = unsafe { c_string(*ptr) };
							println!("- {name}");
						}
					}
					ModelFieldType::U8 => {
						println!("{}: u8", &field.name);
					},
					ModelFieldType::I8 => {
						println!("{}: i8", &field.name);
					},
					ModelFieldType::U16 => {
						println!("{}: u16", &field.name);
					},
					ModelFieldType::I16 => {
						println!("{}: i16", &field.name);
					},
					ModelFieldType::F32 => {
						println!("{}: f32", &field.name);
					},
					ModelFieldType::U32 => {
						println!("{}: u32", &field.name);
					},
					ModelFieldType::I32 => {
						println!("{}: i32", &field.name);
					},
					ModelFieldType::F64 => {
						println!("{}: f64", &field.name);
					},
					ModelFieldType::U64 => {
						println!("{}: u64", &field.name);
					},
					ModelFieldType::I64 => {
						println!("{}: i64", &field.name);
					},
					ModelFieldType::Bool => {
						println!("{}: bool", &field.name);
					},
					ModelFieldType::String => {
						println!("{}: string", &field.name);
					},
					ModelFieldType::Array => todo!(),
					ModelFieldType::Unknown => {
						println!("{}: unknown", &field.name);
					}
				}

				for constraint in &field.meta.constraints {
					match constraint {
						ModelFieldConstraint::Length { value } => println!("+ Length({value})"),
						ModelFieldConstraint::PrimaryKey => println!("+ PrimaryKey"),
						ModelFieldConstraint::Link { dst_table, dst_column, delete, update } => println!("+ Link to {dst_table}.{dst_column}, {delete:?} on delete, {update:?} on update"),
					}
				}
			}

			for fields in model.get_unique_keys() {
				println!("* Unique key {:?}", &fields);
			}

			println!("TODO: create table");
			return Ok(());
		}
	};

	println!("Schema: {:?}", &schema);
	return Ok(());
}

// TODO: DRY
#[allow(non_camel_case_types)]
pub type c_str = *const std::ffi::c_char;

pub unsafe fn c_string (ptr: c_str) -> String {
	use std::ptr::slice_from_raw_parts;
	let raw = &*slice_from_raw_parts(ptr as *const u8, c_strlen(ptr));
	return String::from_utf8_lossy(raw).into_owned();
}

pub fn c_strlen (raw: c_str) -> usize {
	if raw.is_null() {
        return 0;
    }

    let mut len = 0;
    unsafe {
        let mut ptr = raw;
        while *ptr != 0 {
            len += 1;
            ptr = ptr.add(1);
        }
    }

    return len;
}
