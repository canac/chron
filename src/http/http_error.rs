use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use core::fmt;
use std::error::Error;

#[derive(Debug)]
pub enum HttpError {
    GenericError(StatusCode),
}

// HttpError essentially wraps a StatusCode
impl HttpError {
    pub fn from_status_code(status_code: StatusCode) -> Self {
        Self::GenericError(status_code)
    }

    fn reason(&self) -> String {
        match self {
            Self::GenericError(status_code) => status_code
                .canonical_reason()
                .unwrap_or("unknown")
                .to_owned(),
        }
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::GenericError(status_code) => {
                write!(f, "{}: {}", status_code.as_str(), self.reason())
            }
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
            Self::GenericError(status_code) => *status_code,
        }
    }
}
