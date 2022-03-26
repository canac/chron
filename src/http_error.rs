use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use core::fmt;
use std::error::Error;

#[derive(Debug)]
pub struct HttpError(StatusCode);

// HttpError essentially wraps a StatusCode
impl HttpError {
    pub fn new(status_code: StatusCode) -> HttpError {
        HttpError(status_code)
    }

    fn reason(&self) -> &'static str {
        self.0.canonical_reason().unwrap_or("unknown")
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.0.as_str(), self.reason())
    }
}

impl Error for HttpError {
    fn description(&self) -> &str {
        self.0.as_str()
    }
}

impl ResponseError for HttpError {
    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.0).body(self.reason())
    }

    fn status_code(&self) -> StatusCode {
        self.0
    }
}
