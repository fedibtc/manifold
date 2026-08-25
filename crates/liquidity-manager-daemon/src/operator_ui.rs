//! FLIP dashboard assets embedded by the Nix release build.

use axum::Router;

#[derive(rust_embed::Embed)]
#[folder = "$FLIP_OPERATOR_UI_DIST_DIR"]
struct Assets;

pub(crate) fn router() -> Router {
    operator_ui_static::router::<Assets>(&["admin", "health"])
}
