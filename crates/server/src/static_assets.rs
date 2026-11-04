use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web-dist/"]
struct Assets;

pub async fn index() -> Response {
    embedded_file("index.html", "no-cache")
}

pub fn asset(path: &str) -> Response {
    embedded_file(path, "public, max-age=31536000, immutable")
}

fn embedded_file(path: &str, cache: &'static str) -> Response {
    match Assets::get(path) {
        Some(file) => (
            [
                (
                    header::CONTENT_TYPE,
                    mime_guess::from_path(path).first_or_octet_stream().as_ref(),
                ),
                (header::CACHE_CONTROL, cache),
            ],
            file.data.into_owned(),
        )
            .into_response(),
        None => crate::app::error(StatusCode::NOT_FOUND, "not_found", "Resource not found"),
    }
}
