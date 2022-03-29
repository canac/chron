use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use core::fmt;
use std::error::Error;

#[derive(Debug)]
pub enum HttpError {
    GenericError(StatusCode),
    DatabaseError(anyhow::Error),
}

// HttpError essentially wraps a StatusCode
impl HttpError {
    pub fn from_status_code(status_code: StatusCode) -> HttpError {
        HttpError::GenericError(status_code)
    }

    pub fn from_database_error(error: anyhow::Error) -> HttpError {
        HttpError::DatabaseError(error)
    }

    fn reason(&self) -> String {
        match self {
            HttpError::GenericError(status_code) => status_code
                .canonical_reason()
                .unwrap_or("unknown")
                .to_string(),
            HttpError::DatabaseError(err) => format!("{}", err),
        }
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            HttpError::GenericError(status_code) => {
                write!(f, "{}: {}", status_code.as_str(), self.reason())
            }
            HttpError::DatabaseError(err) => write!(f, "{}", err),
        }
    }
}

impl Error for HttpError {}

impl ResponseError for HttpError {
    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.status_code()).body(self.reason())
    }

    fn status_code(&self) -> StatusCode {
        match self {
            HttpError::GenericError(status_code) => *status_code,
            HttpError::DatabaseError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
