mod desktop;

use std::fs::OpenOptions;
use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt;

use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gio::prelude::*;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::service::TowerToHyperService;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::net::unix::pipe;
use tokio::process;

const PORT_PATH: &str = "/dev/virtio-ports/bubbles-agent";
const READY_LINE: &[u8] = b"BUBBLES-AGENT-READY\n";

fn main() {
    tokio::runtime::Runtime::new()
        .expect("a tokio runtime")
        .block_on(serve());
}

async fn serve() {
    let port: OwnedFd = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(PORT_PATH)
        .expect("the agent port to open")
        .into();
    let app = Router::new()
        .route("/shutdown", post(shutdown))
        .route("/spawn-terminal", post(spawn_terminal))
        .route("/desktop-apps", get(desktop_apps))
        .route("/start-desktop-app", post(start_desktop_app));

    let reader = pipe::Receiver::from_owned_fd_unchecked(
        port.try_clone().expect("a copy of the agent port"),
    )
    .expect("agent port reader");
    let mut writer = pipe::Sender::from_owned_fd_unchecked(port).expect("agent port writer");

    writer.write_all(READY_LINE).await.expect("the ready line to be written");
    let io = TokioIo::new(tokio::io::join(reader, writer));
    hyper::server::conn::http2::Builder::new(TokioExecutor::new())
        .serve_connection(io, TowerToHyperService::new(app))
        .await
        .expect("the connection to the host to last");
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
