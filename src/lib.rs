use std::error::Error;
use std::fmt::{Display, Formatter};

pub mod backup;
pub mod bptree;
pub mod catalog;
pub mod index;
pub mod server;
pub mod sql;
pub mod storage;

pub type DbResult<T> = Result<T, DbError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbError {
    message: String,
}

impl DbError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for DbError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for DbError {}

impl From<std::io::Error> for DbError {
    fn from(value: std::io::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<std::num::ParseIntError> for DbError {
    fn from(value: std::num::ParseIntError) -> Self {
        Self::new(value.to_string())
    }
}

impl From<std::num::ParseFloatError> for DbError {
    fn from(value: std::num::ParseFloatError) -> Self {
        Self::new(value.to_string())
    }
}

impl From<std::string::FromUtf8Error> for DbError {
    fn from(value: std::string::FromUtf8Error) -> Self {
        Self::new(value.to_string())
    }
}
