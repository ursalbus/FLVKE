use jsonwebtoken::{decode, DecodingKey, Validation, Algorithm};
use warp::{Filter, Rejection};

use super::state::AppState;
use super::models::{Claims, AuthQuery};
use super::errors::AuthError;

// Function to validate the JWT
pub fn validate_token(token: &str, secret: &str) -> Result<Claims, String> {
    let key = DecodingKey::from_secret(secret.as_ref());
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true; // Check expiration
    validation.set_audience(&["authenticated"]); // Verify audience

    decode::<Claims>(token, &key, &validation)
        .map(|data| data.claims)
        .map_err(|err| format!("JWT validation failed: {}", err))
}

// Warp filter to extract token, validate it, and pass user_id
pub fn with_auth(
    state: AppState,
) -> impl Filter<Extract = (String,), Error = Rejection> + Clone {
    warp::query::<AuthQuery>()
        .and(warp::any().map(move || state.clone()))
        .and_then(|query: AuthQuery, current_state: AppState| async move {
            match validate_token(&query.token, &current_state.jwt_secret) {
                Ok(claims) => {
                     if claims.sub.is_empty() {
                         eprintln!("JWT validation error: Missing or empty 'sub' claim.");
                         Err(warp::reject::custom(AuthError::InvalidToken))
                     } else {
                        println!("JWT validated for user: {}", claims.sub);
                        Ok(claims.sub)
                     }
                }
                Err(e) => {
                    eprintln!("JWT validation error: {}", e);
                    Err(warp::reject::custom(AuthError::InvalidToken))
                }
            }
        })
} 