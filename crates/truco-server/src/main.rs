use std::{net::SocketAddr, str::FromStr};

use tokio::net::TcpListener;
use truco_server::{app, AppState};

#[tokio::main]
async fn main() {
    let bind = std::env::var("TRUCO_SERVER_BIND")
        .or_else(|_| std::env::var("TRUCO_ENGINE_SERVICE_BIND"))
        .unwrap_or_else(|_| "127.0.0.1:4000".to_string());
    let addr = SocketAddr::from_str(&bind).expect("invalid TRUCO_SERVER_BIND");
    let listener = TcpListener::bind(addr)
        .await
        .expect("failed to bind Truco server listener");

    axum::serve(listener, app(AppState::default()))
        .await
        .expect("Truco server failed");
}
