use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderName, HeaderValue, StatusCode, header},
    routing::{any, get},
};
use serde::Serialize;
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

pub mod api;
pub mod attachment;
pub mod auth;
pub mod build;
pub mod clock;
pub mod config;
pub mod crypto;
pub mod deployment;
pub mod relay;
pub mod runtime;
pub mod service;
pub mod ssh;
pub mod storage;
pub mod tmux;

mod generated {
    pub mod contracts;
}

#[derive(Clone, Copy)]
struct FoundationState {
    build: build::BuildMetadata,
}

#[derive(Serialize)]
struct StatusResponse {
    service: &'static str,
    stage: &'static str,
    status: &'static str,
    build: build::BuildMetadata,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
}

/// Build the foundation-only router used by focused HTTP tests.
pub fn app(web_dir: impl Into<PathBuf>) -> Router {
    let web = web_service(web_dir.into());
    Router::new()
        .route("/health", get(foundation_health))
        .route("/ready", get(foundation_ready))
        .route("/api", any(api_not_found))
        .route("/api/", any(api_not_found))
        .route("/api/{*path}", any(api_not_found))
        .route("/auth", any(api_not_found))
        .route("/auth/", any(api_not_found))
        .route("/auth/{*path}", any(api_not_found))
        .fallback_service(web)
        .layer(TraceLayer::new_for_http())
        .with_state(FoundationState {
            build: build::metadata(),
        })
}

/// Build the single-node product router after durable bootstrap and node registration.
pub fn product_app(web_dir: impl Into<PathBuf>, state: Arc<service::ServerState>) -> Router {
    debug_assert_eq!(generated::contracts::PUBLIC_CONTRACT_VERSION, "public.v1");
    debug_assert!(generated::contracts::CONTROL_SCHEMA_ID.ends_with("control.schema.json"));
    debug_assert!(generated::contracts::STATUS_VALUES.contains(&"ready"));
    debug_assert!(generated::contracts::ERROR_CODES.contains(&"temporarily_unavailable"));
    debug_assert_eq!(
        generated::contracts::RELAY_PROTOCOL_VERSION,
        relay::protocol::VERSION
    );
    debug_assert_eq!(
        generated::contracts::RELAY_MAX_FRAME_BYTES,
        relay::protocol::MAX_FRAME_BYTES
    );
    debug_assert_eq!(
        generated::contracts::RELAY_MAX_DATA_BYTES,
        relay::protocol::MAX_DATA_BYTES
    );
    debug_assert_eq!(
        generated::contracts::RELAY_MAX_STREAMS,
        relay::protocol::MAX_STREAMS
    );
    debug_assert_eq!(
        generated::contracts::ATTACHMENT_CONTRACT_VERSION,
        "attachment.v1"
    );
    debug_assert_eq!(generated::contracts::ATTACHMENT_MAX_FRAME_BYTES, 32_768);
    let web = web_service(web_dir.into());
    Router::new()
        .route("/health", get(product_health))
        .route("/ready", get(product_ready))
        .nest("/api/v1", api::router(Arc::clone(&state)))
        .nest("/relay/v1", relay::router(Arc::clone(&state)))
        .nest("/attachment/v1", attachment::router(Arc::clone(&state)))
        .route("/api", any(api_not_found))
        .route("/api/", any(api_not_found))
        .route("/api/{*path}", any(api_not_found))
        .route("/auth", any(api_not_found))
        .route("/auth/", any(api_not_found))
        .route("/auth/{*path}", any(api_not_found))
        .fallback_service(web)
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(15),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static(
                "camera=(), display-capture=(), geolocation=(), microphone=(), payment=(), usb=()",
            ),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'; object-src 'none'; connect-src 'self'; style-src 'self' 'unsafe-inline'",
            ),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn web_service(
    web_dir: PathBuf,
) -> tower_http::services::ServeDir<tower_http::services::ServeFile> {
    let index = web_dir.join("index.html");
    ServeDir::new(web_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(index))
}

async fn foundation_health(State(state): State<FoundationState>) -> Json<StatusResponse> {
    Json(status("foundation", "ok", state.build))
}
async fn foundation_ready(State(state): State<FoundationState>) -> Json<StatusResponse> {
    Json(status("foundation", "ready", state.build))
}
async fn product_health(State(_state): State<Arc<service::ServerState>>) -> Json<StatusResponse> {
    Json(status("single_node", "ok", build::metadata()))
}
async fn product_ready(
    State(state): State<Arc<service::ServerState>>,
) -> (StatusCode, Json<StatusResponse>) {
    let (code, status_value) = if state.lease.is_ready() {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not_ready")
    };
    (
        code,
        Json(status("single_node", status_value, build::metadata())),
    )
}
fn status(
    stage: &'static str,
    status: &'static str,
    build: build::BuildMetadata,
) -> StatusResponse {
    StatusResponse {
        service: "owlmux-server",
        stage,
        status,
        build,
    }
}

async fn api_not_found() -> (StatusCode, Json<ErrorBody>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorBody {
            code: "not_implemented",
            message: "This OwlMux route is not implemented.",
        }),
    )
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request, header},
    };
    use serde_json::Value;
    use tower::ServiceExt as _;

    use super::*;

    #[tokio::test]
    async fn health_uses_the_reviewed_foundation_shape() {
        let response = app("missing-web-directory")
            .oneshot(
                Request::get("/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.expect("body");
        let body: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(body["service"], "owlmux-server");
        assert_eq!(body["stage"], "foundation");
        assert_eq!(body["status"], "ok");
        assert_eq!(body["build"]["id"], build::BUILD_ID);
        assert_eq!(body.as_object().expect("object").len(), 4);
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
                "{path}"
            );
        }
    }
}
