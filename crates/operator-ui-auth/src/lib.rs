pub mod auth;

use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use fedimint_core::hex::ToHex;
use fedimint_core::module::ApiAuth;
use fedimint_core::secp256k1::rand::{Rng, thread_rng};
use serde::Deserialize;

/// Generic state for password-session or trusted-proxy authentication.
#[derive(Clone)]
pub struct UiState<T> {
    pub api: T,
    pub auth_cookie_name: String,
    pub auth_cookie_value: String,
    /// Whether requests require a session cookie. When false, an
    /// authenticating deployment proxy is the operator identity boundary.
    pub requires_auth: bool,
}

impl<T> UiState<T> {
    pub fn new(api: T, requires_auth: bool) -> Self {
        Self {
            api,
            auth_cookie_name: thread_rng().r#gen::<[u8; 4]>().encode_hex(),
            auth_cookie_value: thread_rng().r#gen::<[u8; 32]>().encode_hex(),
            requires_auth,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginInput {
    pub password: String,
}

/// Verify a password and, on success, add the authenticated session cookie.
pub fn authenticate_password(
    auth: &ApiAuth,
    auth_cookie_name: String,
    auth_cookie_value: String,
    jar: CookieJar,
    input: &LoginInput,
) -> Option<CookieJar> {
    if !auth.verify(&input.password) {
        return None;
    }

    let mut cookie = Cookie::new(auth_cookie_name, auth_cookie_value);
    cookie.set_http_only(true);
    cookie.set_same_site(Some(SameSite::Lax));
    Some(jar.add(cookie))
}
