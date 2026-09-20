use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Room not found")]
    RoomNotFound,

    #[error("Too many rooms")]
    TooManyRooms,

    #[error("Room is full")]
    RoomFull,

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    #[error("Not authorized")]
    NotAuthorized,

    #[error("Invalid request")]
    InvalidRequest,
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::RateLimitExceeded => StatusCode::TOO_MANY_REQUESTS,
            AppError::NotAuthorized => StatusCode::FORBIDDEN,
            AppError::RoomNotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status(), self.to_string()).into_response()
    }
}
