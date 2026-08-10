use std::path::PathBuf;

use axum::{
    Json, Router,
    http::StatusCode,
    routing::{any, get},
};
use serde::Serialize;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

#[derive(Serialize)]
struct StatusResponse {
    service: &'static str,
    stage: &'static str,
    status: &'static str,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
}

pub fn app(web_dir: impl Into<PathBuf>) -> Router {
    let web_dir = web_dir.into();
    let index = web_dir.join("index.html");
    let web = ServeDir::new(web_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(index));

    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/api", any(api_not_found))
        .route("/api/", any(api_not_found))
        .route("/api/{*path}", any(api_not_found))
        .route("/auth", any(api_not_found))
        .route("/auth/", any(api_not_found))
        .route("/auth/{*path}", any(api_not_found))
        .fallback_service(web)
        .layer(TraceLayer::new_for_http())
}

async fn health() -> Json<StatusResponse> {
    Json(StatusResponse {
        service: "owlmux-server",
        stage: "foundation",
        status: "ok",
    })
}

async fn ready() -> Json<StatusResponse> {
    Json(StatusResponse {
        service: "owlmux-server",
        stage: "foundation",
        status: "ready",
    })
}

async fn api_not_found() -> (StatusCode, Json<ErrorBody>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorBody {
            code: "not_implemented",
            message: "This OwlMux API is not implemented in the foundation build.",
        }),
    )
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, header},
    };
    use tower::ServiceExt as _;

    use super::*;

    #[tokio::test]
    async fn health_is_available_without_product_services() {
        let response = app("missing-web-directory")
            .oneshot(
                Request::get("/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_and_auth_paths_do_not_fall_back_to_the_spa() {
        let app = app("missing-web-directory");
        let requests = [
            (Method::GET, "/api"),
            (Method::GET, "/api/"),
            (Method::GET, "/api/v1/machines"),
            (Method::POST, "/api/"),
            (Method::GET, "/auth"),
            (Method::GET, "/auth/"),
            (Method::POST, "/auth/v1/session/actions/create"),
        ];

        for (method, path) in requests {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");

            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(
                response.headers().get(header::CONTENT_TYPE),
                Some(&header::HeaderValue::from_static("application/json")),
                "{path}",
            );
        }
    }
}
