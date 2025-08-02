use axum::response::{IntoResponse, Response};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Room not found")]
    RoomNotFound,

    #[error("Too many rooms")]
    TooManyRooms,

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    #[error("Not authorized")]
    NotAuthorized,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (axum::http::StatusCode::BAD_REQUEST, self.to_string()).into_response()
    }
}
