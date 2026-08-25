mod desktop;

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use gio::prelude::*;
use tokio::process;

const TOKEN_CREDENTIAL: &str = "bubbles.agent-token";

#[tokio::main]
async fn main() {
    let token = read_token();
    let app = Router::new()
        .route("/ready", get(ready))
        .route("/shutdown", post(shutdown))
        .route("/spawn-terminal", post(spawn_terminal))
        .route("/desktop-apps", get(desktop_apps))
        .route("/start-desktop-app", post(start_desktop_app))
        .layer(middleware::from_fn_with_state(token, authorize));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:11111")
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}

// The host passes the token as a systemd credential; without one, anyone who
// can reach the port could drive the bubble, so there is no fallback.
fn read_token() -> Arc<[u8]> {
    let directory = std::env::var("CREDENTIALS_DIRECTORY")
        .unwrap_or_else(|_| panic!("no credentials directory, is {TOKEN_CREDENTIAL} loaded?"));
    let token = std::fs::read(std::path::Path::new(&directory).join(TOKEN_CREDENTIAL))
        .unwrap_or_else(|error| panic!("cannot read {TOKEN_CREDENTIAL}: {error}"));
    // An empty token would let a bare "Bearer " through.
    assert!(!token.is_empty(), "{TOKEN_CREDENTIAL} is empty");
    token.into()
}

async fn authorize(State(token): State<Arc<[u8]>>, request: Request, next: Next) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.as_bytes().strip_prefix(b"Bearer "));
    match presented {
        Some(presented) if presented == &token[..] => next.run(request).await,
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

// Fire and forget: the answer only says whether the program could be started.
fn spawn(command: &mut process::Command, started: &str) -> (StatusCode, String) {
    match command.spawn() {
        Ok(_) => (StatusCode::CREATED, started.to_string()),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "failed to start {}: {error}",
                command.as_std().get_program().to_string_lossy()
            ),
        ),
    }
}

async fn ready() -> &'static str {
    "OK"
}

async fn shutdown() -> impl IntoResponse {
    spawn(
        process::Command::new("sudo").args(["shutdown", "-h", "now"]),
        "Shutdown initiated",
    )
}

async fn spawn_terminal() -> impl IntoResponse {
    spawn(&mut process::Command::new("x-terminal-emulator"), "Spawned")
}

async fn desktop_apps() -> impl IntoResponse {
    Json(desktop::list_apps())
}

#[derive(Deserialize)]
struct StartApp {
    app: String,
}

async fn start_desktop_app(Json(params): Json<StartApp>) -> impl IntoResponse {
    // None as well when the entry's TryExec is not found.
    let Some(app) = gio::DesktopAppInfo::new(&params.app) else {
        return (
            StatusCode::NOT_FOUND,
            format!("no application with id {}", params.app),
        );
    };

    // GIO handles field codes and Terminal=true, and fails if Exec cannot run.
    match app.launch(&[], gio::AppLaunchContext::NONE) {
        Ok(()) => (StatusCode::CREATED, "Launched".to_string()),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to start {}: {error}", params.app),
        ),
    }
}
