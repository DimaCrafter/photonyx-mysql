use std::any::Any;

use bindings::{camel_to_kebab, db::{DatabaseConnection, DatabaseImpl, EntityHandle, EntityList, ModelMetaImpl, QueryBuilder}, validator_json};
use json::JsonValue;
use mysql::{prelude::Queryable, OptsBuilder, Pool, Row};
use crate::{model::fill_model_entity, query::{SqlPart, SqlQuery}};

mod model;
mod query;


validator_json! {
	ConnectionConfig,
    host: String,
	port: u16,
    name: String,
    username: String,
    password: String
}


struct MySqlConnection {
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
    fn new_query (&self, collection: &str) -> Box<dyn QueryBuilder> {
        let mut table_name = camel_to_kebab(collection);
        if table_name.ends_with('s') {
            table_name.push_str("es");
        } else {
            table_name.push_str("s");
        }

        return Box::new(SqlQuery::new(&table_name));
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

    fn prepare_model (&mut self, model: &mut Box<dyn ModelMetaImpl>) {
        println!("mysql::prepare_model {}", model.get_name());
    }
}


struct MySqlImpl;

impl DatabaseImpl for MySqlImpl {
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

#[unsafe(no_mangle)]
pub fn provide_database () -> Box<dyn DatabaseImpl> {
    return Box::new(MySqlImpl{});
}
