use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use tokio::process;

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/ready", get(ready))
        .route("/shutdown", post(shutdown))
        .route("/spawn-terminal", post(spawn_terminal));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn ready() -> &'static str {
    "OK"
}

async fn shutdown() -> impl IntoResponse {
    process::Command::new("sudo").arg("shutdown").arg("-h").arg("now").spawn().expect("failed to spawn");
    (StatusCode::CREATED, "Shutdown initiated")
}

async fn spawn_terminal() -> impl IntoResponse {
    process::Command::new("x-terminal-emulator").current_dir("/home/user").env("XDG_RUNTIME_DIR", "/run/user/1000").spawn().expect("failed to spawn");
    (StatusCode::CREATED, "Spawned")
}
