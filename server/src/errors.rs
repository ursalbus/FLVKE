use warp::{http::StatusCode, reject, Rejection, Reply};
use std::convert::Infallible;

// Custom Auth Rejection
#[derive(Debug)]
pub enum AuthError {
    InvalidToken,
}

impl reject::Reject for AuthError {}

// Warp Rejection Handler
pub async fn handle_rejection(err: Rejection) -> Result<impl Reply, Infallible> {
    eprintln!("Handling rejection: {:?}", err);

    if err.is_not_found() {
        Ok(warp::reply::with_status("NOT_FOUND", StatusCode::NOT_FOUND))
    } else if let Some(_) = err.find::<AuthError>() {
        Ok(warp::reply::with_status(
            "UNAUTHORIZED",
            StatusCode::UNAUTHORIZED,
        ))
    } else if let Some(_) = err.find::<reject::MethodNotAllowed>() {
        Ok(warp::reply::with_status(
            "METHOD_NOT_ALLOWED",
            StatusCode::METHOD_NOT_ALLOWED,
        ))
     } else if err.find::<reject::InvalidQuery>().is_some() {
         Ok(warp::reply::with_status(
             "BAD_REQUEST - Missing or invalid token query parameter",
             StatusCode::BAD_REQUEST,
         ))
    } else {
        eprintln!("Unhandled rejection type, returning 500: {:?}", err);
        Ok(warp::reply::with_status(
            "INTERNAL_SERVER_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
} 