use bindings::session::{SessionStoreImpl, SessionStoreMeta};
use mysql::prelude::Queryable;
use crate::connection::MySqlConnection;


pub struct MySqlSessionStoreMeta;
impl SessionStoreMeta for MySqlSessionStoreMeta {
	fn get_id (&self) -> String {
		return "mysql".to_owned();
	}

	fn new_impl (&self, db_conf_name: &str) -> Option<Box<dyn SessionStoreImpl>> {
		let conn = bindings::db::find_db_connection(db_conf_name)?;
		let mysql = match conn.as_any().downcast_ref::<MySqlConnection>() {
			Some(value) => value,
			None => {
				println!("'{db_conf_name}' is not the MySQL configuration");
				return None;
			}
		};

		return Some(Box::new(MySqlSessionStoreImpl(mysql)));
	}
}

pub struct MySqlSessionStoreImpl(&'static MySqlConnection);

impl SessionStoreImpl for MySqlSessionStoreImpl {
	fn create (&self, inner: &mut json::JsonValue, expires: f64) {
		// todo: add error handling
		let mut conn = self.0.get_conn().unwrap();

		conn.exec_drop("INSERT INTO `sessions` (`expires`, `data`) VALUES (?, ?)", (expires, inner["data"].dump())).unwrap();
		inner["id"] = conn.last_insert_id().into();
	}

	fn update (&self, inner: &mut json::JsonValue) {
		// todo: add error handling
		let mut conn = self.0.get_conn().unwrap();

		conn.exec_drop("UPDATE `sessions` SET `data` = ? WHERE `id` = ?", (inner["data"].dump(), inner["id"].as_u64().unwrap())).unwrap();
	}

	fn destroy (&self, inner: &mut json::JsonValue) {
		// todo: add error handling
		let mut conn = self.0.get_conn().unwrap();

		let id = inner.remove("id").as_u64().unwrap();
		conn.exec_drop("DELETE FROM `sessions` WHERE `id` = ?", (id,)).unwrap();
	}
}
