use std::{ffi::c_void, ptr::null};
use bindings::{c::{disown, Slice}, db::{EntityHandle, ModelField, ModelFieldType, ModelMeta}};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use mysql::{from_value_opt, prelude::FromValue, Row, Value};


pub fn fill_model_entity (model: &ModelMeta, row: Row, entity: EntityHandle) -> Result<(), String> {
	let columns = row.columns_ref();

	for field in &model.fields {
		let index = match columns.iter().position(|column| column.name_str() == field.column.as_str()) {
			Some(index) => index,
			None => continue
		};

		let value = match row.as_ref(index) {
			Some(Value::NULL) | None => {
				(field.setter)(entity, null());
				continue;
			}
			Some(value) => value
		};

		match field.type_id {
			ModelFieldType::U8 => set_number::<u8>(value, entity, field)?,
			ModelFieldType::I8 => set_number::<i8>(value, entity, field)?,
			ModelFieldType::U16 => set_number::<u16>(value, entity, field)?,
			ModelFieldType::I16 => set_number::<i16>(value, entity, field)?,
			ModelFieldType::U32 => set_number::<u32>(value, entity, field)?,
			ModelFieldType::I32 => set_number::<i32>(value, entity, field)?,
			ModelFieldType::F32 => set_number::<f32>(value, entity, field)?,
			ModelFieldType::U64 => set_number::<u64>(value, entity, field)?,
			ModelFieldType::I64 => set_number::<i64>(value, entity, field)?,
			ModelFieldType::F64 => set_number::<f64>(value, entity, field)?,
			ModelFieldType::Bool => {
				// `bool` is `tinyint(1)`: any non-zero value is true
				let number = parse_integer(value).ok_or_else(|| convert_error(field, value))?;
				let flag = number != 0;
				(field.setter)(entity, &flag as *const bool as *const c_void);
			}
			ModelFieldType::String | ModelFieldType::Json => {
				// JSON is passed as its text
				let raw = value_to_bytes(value);
				let slice = Slice::for_vec(&raw);
				(field.setter)(entity, &slice as *const Slice<u8> as *const c_void);
			}
			ModelFieldType::DateTime => {
				let parsed = parse_datetime(value).ok_or_else(|| convert_error(field, value))?;
				(field.setter)(entity, disown(parsed) as *const c_void);
			}
			ModelFieldType::Enum => {
				let ordinal = enum_ordinal(field, value).ok_or_else(|| {
					format!("Field '{}': value {} is not a value of the enum", &field.name, value.as_sql(true))
				})?;
				(field.setter)(entity, &ordinal as *const u32 as *const c_void);
			}
		}
	}

	return Ok(());
}

fn convert_error (field: &ModelField, value: &Value) -> String {
	return format!("Field '{}': failed to convert {} into {}", &field.name, value.as_sql(true), field.storage);
}

fn set_number<T: FromValue> (value: &Value, entity: EntityHandle, field: &ModelField) -> Result<(), String> {
	let number: T = from_value_opt(value.clone()).map_err(|_| convert_error(field, value))?;
	(field.setter)(entity, &number as *const T as *const c_void);
	return Ok(());
}

fn parse_integer (value: &Value) -> Option<i128> {
	return match value {
		Value::Int(number) => Some(*number as i128),
		Value::UInt(number) => Some(*number as i128),
		Value::Bytes(raw) => std::str::from_utf8(raw).ok()?.trim().parse().ok(),
		_ => None
	};
}

fn value_to_bytes (value: &Value) -> Vec<u8> {
	return match value {
		Value::Bytes(raw) => raw.clone(),
		Value::Int(number) => number.to_string().into_bytes(),
		Value::UInt(number) => number.to_string().into_bytes(),
		Value::Float(number) => number.to_string().into_bytes(),
		Value::Double(number) => number.to_string().into_bytes(),
		other => {
			let sql = other.as_sql(true);
			sql.trim_matches('\'').as_bytes().to_owned()
		}
	};
}

fn parse_datetime (value: &Value) -> Option<DateTime<Utc>> {
	match value {
		Value::Date(year, month, day, hour, min, sec, micros) => {
			let datetime = Utc.with_ymd_and_hms(*year as i32, *month as u32, *day as u32, *hour as u32, *min as u32, *sec as u32);
			if let chrono::offset::LocalResult::Single(parsed) = datetime {
				return Some(parsed + chrono::Duration::microseconds(*micros as i64));
			}

			return None;
		}
		// text protocol
		Value::Bytes(raw) => {
			let text = std::str::from_utf8(raw).ok()?;
			for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
				if let Ok(parsed) = NaiveDateTime::parse_from_str(text, format) {
					return Some(Utc.from_utc_datetime(&parsed));
				}
			}

			return None;
		}
		_ => {
			return None;
		}
	}
}

fn enum_ordinal (field: &ModelField, value: &Value) -> Option<u32> {
	return match value {
		Value::Bytes(raw) => {
			let text = std::str::from_utf8(raw).ok()?;
			if let Some(ordinal) = field.enum_ordinal_by_str(text) {
				return Some(ordinal);
			}

			// Numeric storage read by the text protocol
			field.enum_ordinal_by_int(text.trim().parse().ok()?)
		}
		Value::Int(number) => {
			field.enum_ordinal_by_int(*number)
		}
		Value::UInt(number) => {
			field.enum_ordinal_by_int(i64::try_from(*number).ok()?)
		}
		_ => None
	};
}
