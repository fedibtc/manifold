//! FMan dashboard assets embedded by the Nix release build.

use axum::Router;

#[derive(rust_embed::Embed)]
#[folder = "$FMAN_OPERATOR_UI_DIST_DIR"]
struct Assets;

pub fn router() -> Router {
    operator_ui_static::router::<Assets>(&["api"])
}
