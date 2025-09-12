use bindings::{db::DatabaseImpl, session::SessionStoreMeta};
use crate::{connection::MySqlImpl, session::MySqlSessionStoreMeta};

mod connection;
mod model;
mod query;
mod schema;
mod session;


#[unsafe(no_mangle)]
pub fn provide_database () -> Box<dyn DatabaseImpl> {
    return Box::new(MySqlImpl {});
}

#[unsafe(no_mangle)]
pub fn provide_session_store () -> Box<dyn SessionStoreMeta> {
    return Box::new(MySqlSessionStoreMeta);
}
