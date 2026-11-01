use std::any::Any;

use bindings::{c::Slice, db::QueryBuilder};
use json::{object::Object, Array, JsonValue};
use mysql::Value;

// todo: better SqlPart error tracing (like invalid op. or unsupported type)
#[derive(Clone)]
pub struct SqlPart {
	pub raw: String,
	pub params: Vec<Value>
}

fn append_name (raw: &mut String, name: &str) {
	raw.push_str("`");
	if let Some((a, b)) = name.split_once('.') {
		raw.push_str(a);
		raw.push_str("`.`");
		raw.push_str(b);
	} else {
		raw.push_str(name);
	}
	raw.push_str("`");
}

impl SqlPart {
	pub fn new () -> Self {
		SqlPart { raw: String::new(), params: Vec::new() }
	}

	pub fn append<S: AsRef<str>> (&mut self, next_part: S) {
		self.raw.push_str(next_part.as_ref());
	}

	pub fn append_value (&mut self, value: &JsonValue) {
		let native = match value {
			JsonValue::Null => Value::NULL,
			JsonValue::Short(value) => Value::Bytes(value.as_bytes().to_owned()),
			JsonValue::String(value) => Value::Bytes(value.as_bytes().to_owned()),
			JsonValue::Number(_) => Value::Double(value.as_f64().unwrap()),
			JsonValue::Boolean(value) => Value::from(value),
			JsonValue::Object(_) => {
				println!("Unsupported SQL value: object");
				Value::NULL
			},
			JsonValue::Array(values) => {
				let mut first = true;
				for item in values {
					if first {
						first = false;
					} else {
						self.raw.push_str(", ");
					}

					self.append_value(item);
				}

				return;
			},
		};

		self.raw.push_str("?");
		self.params.push(native);
	}

	pub fn append_part (&mut self, mut value: SqlPart) {
		self.raw.push_str(&value.raw);
		self.params.append(&mut value.params);
	}


	pub fn query (&mut self, query: &JsonValue) {
		if let JsonValue::Object(query_obj) = query {
			self.push_query(query_obj);
		} else {
			if let JsonValue::Array(array) = query {
				if array.is_empty() {
					return;
				}
			}

			println!("Invalid where: expected object");
		}
	}

	fn push_query (&mut self, query: &Object) {
		let mut first = true;
		for (key, value) in query.iter() {
			if first {
				first = false;
			} else {
				self.append(" AND ");
			}

			match key {
				"$or" => {
					if let JsonValue::Array(variants) = value {
						self.append("(");
						self.push_query_stmt(" OR ", variants);
						self.append(")");
					} else {
						println!("Invalid $or: expected array");
					}
				}
				"$and" => {
					if let JsonValue::Array(variants) = value {
						self.push_query_stmt(" AND ", variants);
					} else {
						println!("Invalid $and: expected array");
					}
				}
				_ => {
					append_name(&mut self.raw, key);
					if let Some((cmp_name, cmp_arg)) = match_cmp(value) {
						self.push_value_cmp(cmp_name, cmp_arg);
					} else {
						if let JsonValue::Null = value {
							self.append(" IS NULL");
						} else {
							self.append(" = ");
							self.append_value(value);
						}
					}
				}
			}
		}
	}

	fn push_query_stmt (&mut self, separator: &str, variants: &Array) {
		let mut first = true;
		for variant in variants {
			if let JsonValue::Object(variant) = variant {
				let mut variant_part = SqlPart::new();
				variant_part.push_query(variant);

				if first {
					first = false;
				} else {
					self.append(separator);
				}

				self.append("(");
				self.append_part(variant_part);
				self.append(")");
			} else {
				println!("Invalid{separator}subquery: expected object");
			}
		}
	}

	fn push_value_cmp (&mut self, name: &str, arg: &JsonValue) {
		match name {
			"$raw" => {
				self.append(" = ");
				self.append(arg.as_str().unwrap());
			}
			"$neq" => {
				if let JsonValue::Null = arg {
					self.append(" IS NOT NULL");
				} else {
					self.append(" <> ");
					self.append_value(arg);
				}
			}
			"$in" => {
				self.append(" IN (");
				self.append_value(arg);
				self.append(")");
			}
			"$gt" => {
				self.append(" > ");
				self.append_value(arg);
			}
			"$lt" => {
				self.append(" < ");
				self.append_value(arg);
			}
			"$gte" => {
				self.append(" >= ");
				self.append_value(arg);
			}
			"$lte" => {
				self.append(" <= ");
				self.append_value(arg);
			}
			"$startsWith" => {
				self.append(" LIKE ");
				let like_arg = format!("{}%", arg.as_str().unwrap_or("").replace('%', "\\%"));
				self.append_value(&JsonValue::String(like_arg));
			}
			"$contains" => {
				self.append(" LIKE ");
				let like_arg = format!("%{}%", arg.as_str().unwrap_or("").replace('%', "\\%"));
				self.append_value(&JsonValue::String(like_arg));
			}
			"$endsWith" => {
				self.append(" LIKE ");
				let like_arg = format!("%{}", arg.as_str().unwrap_or("").replace('%', "\\%"));
				self.append_value(&JsonValue::String(like_arg));
			}
			_ => {
				println!("Unsupported value compare: {name}");
			}
		}
	}


	pub fn limit (&mut self, offset: u32, length: u32) {
		self.append(format!(" LIMIT {offset}, {length}"));
	}

	pub fn order_by (&mut self, key: &str, descending: bool) {
		self.append(" ORDER BY ");
		append_name(&mut self.raw, key);

		if descending {
			self.append(" DESC");
		} else {
			self.append(" ASC");
		}
	}
}

fn match_cmp (maybe_op: &JsonValue) -> Option<(&str, &JsonValue)> {
	if let JsonValue::Object(maybe_op_obj) = maybe_op {
		let mut iter = maybe_op_obj.iter();
		if let Some((key, value)) = iter.next() {
			if key.starts_with('$') {
				return Some((key, value));
			}
		}
	}

	return None;
}

struct SqlSelect {
	raw: String,
	first: bool
}

impl SqlSelect {
	pub fn new () -> Self {
		SqlSelect { raw: String::new(), first: true }
	}

	#[inline]
	fn append_sep (&mut self) {
		if self.first {
			self.first = false;
		} else {
			self.raw.push_str(", ");
		}
	}

	pub fn add_key<S: AsRef<str>> (&mut self, key: S) {
		self.append_sep();
		append_name(&mut self.raw, key.as_ref());
	}

	pub fn add_raw<S: AsRef<str>> (&mut self, raw: S) {
		self.append_sep();
		self.raw.push_str(raw.as_ref());
	}

	pub fn add_as<S: AsRef<str>> (&mut self, raw: S, key: S) {
		self.add_raw(raw);
		self.raw.push_str(" AS ");
		append_name(&mut self.raw, key.as_ref());
	}
}

pub struct SqlQuery {
	table: String,
	select_part: SqlSelect,
	where_part: SqlPart,
	filter_part: SqlPart
}

impl SqlQuery {
	pub fn new (table_name: &str) -> Self {
		SqlQuery {
			table: table_name.to_owned(),
			select_part: SqlSelect::new(),
			where_part: SqlPart::new(),
			filter_part: SqlPart::new()
		}
	}


	pub fn build (&self) -> SqlPart {
		let mut result = SqlPart::new();

		result.append("SELECT ");
		if self.select_part.raw.is_empty() {
			result.append("*");
		} else {
			result.append(&self.select_part.raw);
		}

		result.append(" FROM ");
		append_name(&mut result.raw, &self.table);

		if !self.where_part.raw.is_empty() {
			result.append(" WHERE ");
			result.append_part(self.where_part.clone());
		}

		result.append_part(self.filter_part.clone());

		return result;
	}
}

impl QueryBuilder for SqlQuery {
	fn count (&mut self, query: &JsonValue) {
		self.select_part.add_raw("COUNT(*)");
		self.where_part.query(query);
	}

	fn build (&self) -> Box<dyn Any> {
		return Box::new(SqlQuery::build(self));
	}

	// SAFETY: caller MUST provide UTF-8 string
	fn select (&mut self, projection: &Slice<Slice<u8>>) {
		for key in projection {
			self.select_part.add_key(unsafe { key.as_str_unchecked() });
		}
	}

	fn select_raw (&mut self, stmt: &str) {
		self.select_part.add_raw(stmt);
	}

	fn query (&mut self, query: &JsonValue) {
		let mut part = SqlPart::new();
		part.query(query);
		if part.raw.is_empty() {
			return;
		}

		if !self.where_part.raw.is_empty() {
			self.where_part.append(" AND ");
		}

		self.where_part.append_part(part);
	}

	fn limit (&mut self, offset: u32, length: u32) {
		self.filter_part.limit(offset, length);
	}

	fn debug (&self) {
		println!("Debugging query state:");

		if self.select_part.raw.is_empty() {
			println!("SELECT *");
		} else {
			println!("SELECT {}", &self.select_part.raw);
		}

		println!("FROM {}", &self.table);

		if !self.where_part.raw.is_empty() {
			println!("WHERE {}", &self.where_part.raw);
			println!("-- {:?}", &self.where_part.params);
		}

		if !self.filter_part.raw.is_empty() {
			println!("{}", &self.filter_part.raw);
			println!("-- {:?}", &self.filter_part.params);
		}
	}
}
