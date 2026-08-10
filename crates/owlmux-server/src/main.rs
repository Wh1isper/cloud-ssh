use std::{env, error::Error, net::SocketAddr, path::PathBuf};

use tokio::{net::TcpListener, signal};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init()?;

    let address = env::var("OWLMUX_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_owned())
        .parse::<SocketAddr>()?;
    let web_dir =
        env::var_os("OWLMUX_WEB_DIR").map_or_else(|| PathBuf::from("apps/web/dist"), PathBuf::from);

    let listener = TcpListener::bind(address).await?;
    info!(%address, web_dir = %web_dir.display(), "starting OwlMux foundation server");

    axum::serve(listener, owlmux_server::app(web_dir))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = signal::ctrl_c().await;
}
