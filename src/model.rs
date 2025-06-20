use std::{ffi::c_void, ptr::null};
use bindings::{c::Slice, db::{EntityHandle, ModelField, ModelFieldType, ModelMetaImpl}};
use mysql::{prelude::FromValue, Row};


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

fn eject_value<T: FromValue> (row: &mut Row, key: &str) -> Result<T, String> {
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
