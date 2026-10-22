use std::any::Any;
use bindings::{db::{DatabaseConnection, DatabaseImpl, EntityHandle, EntityList, ModelMetaImpl, QueryBuilder}, validator_json};
use chrono::{TimeZone, Utc};
use json::JsonValue;
use mysql::{OptsBuilder, Pool, Row, Value, prelude::Queryable};

use crate::{model::{auto_migrate, fill_model_entity, get_table_name}, query::{SqlPart, SqlQuery}};


validator_json! {
	ConnectionConfig,
    host: String,
	port: u16,
    name: String,
    username: String,
    password: String
}


pub struct MySqlConnection {
    pool: Pool
}

impl MySqlConnection {
    pub fn new (config: ConnectionConfig) -> Result<Self, String> {
        if config.name.is_empty() {
            return Err("name: field is required".to_owned());
        }

        if config.username.is_empty() {
            return Err("username: field is required".to_owned());
        }

        let mut opts = OptsBuilder::new()
            .ip_or_hostname(Some(config.host))
            .tcp_port(config.port)
            .db_name(Some(config.name))
            .user(Some(config.username));

        if !config.password.is_empty() {
            opts = opts.pass(Some(config.password));
        }

        return Ok(MySqlConnection {
            pool: Pool::new(opts).map_err(|error| error.to_string())?
        });
    }
}

impl MySqlConnection {
    pub fn get_conn (&self) -> Result<mysql::PooledConn, String> {
        match self.pool.get_conn() {
            Err(error) => {
                return Err(format!("Failed to get MySQL connection from pool: {}", error.to_string()));
            }
            Ok(conn) => {
                return Ok(conn);
            }
        }
    }
}

impl DatabaseConnection for MySqlConnection {
	fn as_any (&self) -> &dyn Any {
		return self;
	}

    fn new_query (&self, collection: &str) -> Box<dyn QueryBuilder> {
        return Box::new(SqlQuery::new(&get_table_name(collection)));
    }

    fn exec_value (&mut self, query_any: Box<dyn Any>) -> Result<Option<Box<dyn Any>>, String> {
        let mut conn = self.get_conn()?;

        let query_box = match query_any.downcast::<SqlPart>() {
            Ok(value) => value,
            Err(_) => return Err("Invalid query type passed".to_owned())
        };

        let result: Option<Row> = match conn.exec_first(&query_box.raw, query_box.params) {
            Ok(value) => value,
            Err(error) => {
                println!("DEBUG: {}", &query_box.raw);
                return Err(error.to_string());
            }
        };

        if let Some(mut row) = result {
            if let Some(value) = row.take::<Value, usize>(0) {
                return Ok(match value {
                    Value::NULL => None,
                    Value::Bytes(items) => Some(Box::new(items)),
                    Value::Int(number) => {
                        let b = Box::new(number) as Box<dyn std::any::Any>;
                        let b1 = b.downcast_ref::<i64>().unwrap();
                        println!("b = {:?}, b1 = {}", &b, b1);
                        Some(b)
                    },
                    Value::UInt(number) => {
                        let b = Box::new(number);
                        Some(b)
                    },
                    Value::Float(number) => Some(Box::new(number)),
                    Value::Double(number) => Some(Box::new(number)),
                    Value::Date(year, month, day, hour, min, sec, _) => {
                        let datetime = Utc.with_ymd_and_hms(year as i32, month as u32, day as u32, hour as u32, min as u32, sec as u32);
                        if let chrono::offset::LocalResult::Single(parsed) = datetime {
                            Some(Box::new(parsed))
                        } else {
                            None
                        }
                    },
                    Value::Time(_, _, _, _, _, _) => None
                });
            }
        }

        return Ok(None);
    }

    fn exec_first (&mut self, model: &Box<dyn ModelMetaImpl>, query_any: Box<dyn Any>) -> Result<Option<EntityHandle>, String> {
        let mut conn = self.get_conn()?;

        let query_box = match query_any.downcast::<SqlPart>() {
            Ok(value) => value,
            Err(_) => return Err("Invalid query type passed".to_owned())
        };

        let result: Option<Row> = match conn.exec_first(&query_box.raw, query_box.params) {
            Ok(value) => value,
            Err(error) => {
                println!("DEBUG: {}", &query_box.raw);
                return Err(error.to_string());
            }
        };

        if let Some(value) = result {
            let entity = model.init_entity();
            fill_model_entity(model, value, entity)?;
            return Ok(Some(entity));
        }

        return Ok(None);
    }

    fn exec_all (&mut self, model: &Box<dyn ModelMetaImpl>, query_any: Box<dyn Any>) -> Result<EntityList, String> {
        let mut conn = self.get_conn()?;

        let query_box = match query_any.downcast::<SqlPart>() {
            Ok(value) => value,
            Err(_) => return Err("Invalid query type passed".to_owned())
        };

        let iter = match conn.exec_iter(&query_box.raw, query_box.params) {
            Ok(value) => value,
            Err(error) => {
                println!("DEBUG: {}", &query_box.raw);
                return Err(error.to_string());
            }
        };

        let mut result = model.init_entity_list();
        for item in iter {
            match item {
                Ok(value) => {
                    let entity = model.init_entity();
                    fill_model_entity(model, value, entity)?;
                    result.push(entity);
                }
                Err(error) => {
                    return Err(error.to_string());
                }
            }
        }

        return Ok(result);
    }

    fn auto_migrate (&mut self, model: &mut Box<dyn ModelMetaImpl>) {
        println!("mysql::auto_migrate {}", model.get_name());

        if let Err(error) = auto_migrate(self, model) {
            println!("Auto-migration failed: {error}");
        }
    }
}


pub struct MySqlImpl;

impl DatabaseImpl for MySqlImpl {
	fn get_id (&self) -> String {
		return "mysql".to_owned();
	}

    fn connect (&self, raw_config: &JsonValue) -> Result<Box<dyn DatabaseConnection>, String> {
        let mut config = ConnectionConfig {
            host: "127.0.0.1".to_owned(),
            port: 3306,
            name: String::new(),
            username: String::new(),
            password: String::new()
        };

        if let Err(error) = config.assign_json(raw_config) {
            return Err(format!("Failed to parse config branch {}: {}", error.path.join("."), error.message));
        }

        return Ok(Box::new(MySqlConnection::new(config)?));
    }
}
