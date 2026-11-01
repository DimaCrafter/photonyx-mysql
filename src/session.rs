use std::time::{SystemTime, UNIX_EPOCH};
use bindings::session::{SessionStoreImpl, SessionStoreMeta};
use json::JsonValue;
use mysql::prelude::Queryable;
use crate::connection::MySqlConnection;


const SESSIONS_TABLE: &str = "photonyx_sessions";

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

		let store = MySqlSessionStoreImpl(mysql);
		if let Err(error) = store.create_table() {
			println!("Session store: failed to create `{SESSIONS_TABLE}`: {error}");
			return None;
		}

		return Some(Box::new(store));
	}
}

pub struct MySqlSessionStoreImpl(&'static MySqlConnection);

impl MySqlSessionStoreImpl {
	fn create_table (&self) -> Result<(), String> {
		let mut conn = self.0.get_conn()?;
		conn.query_drop(format!("\
			CREATE TABLE IF NOT EXISTS `{SESSIONS_TABLE}` (\
				`id` bigint unsigned NOT NULL AUTO_INCREMENT, \
				`expires` bigint unsigned NOT NULL, \
				`data` longtext NOT NULL, \
				PRIMARY KEY (`id`), \
				KEY `expires` (`expires`)\
			) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci"
		)).map_err(|error| error.to_string())?;

		return Ok(());
	}
}

fn now_millis () -> u64 {
	return SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
}

impl SessionStoreImpl for MySqlSessionStoreImpl {
	fn create (&self, data: &JsonValue, expires: u64) -> Result<u64, String> {
		let mut conn = self.0.get_conn()?;

		conn
			.exec_drop(
				format!("DELETE FROM `{SESSIONS_TABLE}` WHERE `expires` <= ?"),
				(now_millis(),)
			)
			.map_err(|error| error.to_string())?;

		conn
			.exec_drop(
				format!("INSERT INTO `{SESSIONS_TABLE}` (`expires`, `data`) VALUES (?, ?)"),
				(expires, data.dump())
			)
			.map_err(|error| error.to_string())?;

		return Ok(conn.last_insert_id());
	}

	fn load (&self, id: u64) -> Result<Option<JsonValue>, String> {
		let mut conn = self.0.get_conn()?;

		let raw: Option<String> = conn
			.exec_first(
				format!("SELECT `data` FROM `{SESSIONS_TABLE}` WHERE `id` = ? AND `expires` > ?"),
				(id, now_millis())
			)
			.map_err(|error| error.to_string())?;

		return match raw {
			Some(text) => json::parse(&text).map(Some).map_err(|error| format!("session {id}: {error}")),
			None => Ok(None)
		};
	}

	fn update (&self, id: u64, data: &JsonValue, expires: u64) -> Result<(), String> {
		let mut conn = self.0.get_conn()?;

		conn
			.exec_drop(
				format!("UPDATE `{SESSIONS_TABLE}` SET `data` = ?, `expires` = ? WHERE `id` = ?"),
				(data.dump(), expires, id)
			)
			.map_err(|error| error.to_string())?;

		return Ok(());
	}

	fn destroy (&self, id: u64) -> Result<(), String> {
		let mut conn = self.0.get_conn()?;

		conn
			.exec_drop(format!("DELETE FROM `{SESSIONS_TABLE}` WHERE `id` = ?"), (id,))
			.map_err(|error| error.to_string())?;

		return Ok(());
	}
}
