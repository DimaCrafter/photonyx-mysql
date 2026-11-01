use bindings::{db::DatabaseImpl, session::SessionStoreMeta};
use photonyx_sql_shared::SqlDialect;
use crate::{connection::MySqlImpl, dialect::MySqlDialect, session::MySqlSessionStoreMeta};

mod connection;
mod dialect;
#[cfg(test)]
mod dialect_tests;
#[cfg(test)]
mod db_tests;
mod introspect;
#[cfg(test)]
mod introspect_tests;
mod model;
mod query;
mod session;


#[unsafe(no_mangle)]
pub fn provide_database () -> Box<dyn DatabaseImpl> {
    return Box::new(MySqlImpl {});
}

#[unsafe(no_mangle)]
pub fn provide_session_store () -> Box<dyn SessionStoreMeta> {
    return Box::new(MySqlSessionStoreMeta);
}

#[unsafe(no_mangle)]
pub fn provide_sql_dialect () -> Box<dyn SqlDialect> {
    return Box::new(MySqlDialect);
}
