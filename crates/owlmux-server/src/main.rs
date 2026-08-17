use std::{error::Error, io, net::SocketAddr};

use owlmux_server::{build, config::Config, runtime::RuntimeTasks, service::ServerState};
use tokio::{net::TcpListener, sync::oneshot};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init()?;

    let config = Config::from_env()?;
    let web_dir = config.web_dir().to_path_buf();
    let shutdown_timeout = config.shutdown_timeout();
    let address = config.address();
    let state = ServerState::bootstrap(config).await?;
    let listener = TcpListener::bind(address).await?;
    let local_address = listener.local_addr()?;
    let tasks = RuntimeTasks::new();
    let cancellation = tasks.cancellation_token();
    let (server_result_tx, server_result_rx) = oneshot::channel();

    info!(address = %local_address, web_dir = %web_dir.display(), server_build_id = build::BUILD_ID, incarnation_id = %state.lease.incarnation_id(), "starting OwlMux server");

    let renewal_state = state.clone();
    let renewal_cancellation = cancellation.clone();
    tasks.spawn(async move {
        renewal_state
            .lease
            .clone()
            .run_renewals(renewal_cancellation)
            .await;
    });
    let fence_state = state.clone();
    let fence_token = state.lease.fence_token();
    let fence_cancellation = cancellation.clone();
    let watcher_cancellation = cancellation.clone();
    tasks.spawn(async move {
        tokio::select! {
            () = fence_token.cancelled() => {
                fence_state.relays.close_all().await;
                fence_cancellation.cancel();
            }
            () = watcher_cancellation.cancelled() => {}
        }
    });
    let app = owlmux_server::product_app(web_dir, state.clone());
    tasks.spawn(async move {
        let result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(cancellation.cancelled_owned())
        .await;
        let _ = server_result_tx.send(result);
    });

    let server_result = tokio::select! {
        signal = shutdown_signal() => {
            signal?;
            info!("shutdown requested");
            None
        }
        result = server_result_rx => Some(result.map_err(|_| io::Error::other("server task ended without a result"))?),
    };

    if let Err(error) = state.lease.begin_drain().await {
        warn!(%error, "node could not commit draining state; local authority remains fenced by its deadline");
    }
    state.relays.close_all().await;
    if !tasks.shutdown(shutdown_timeout).await {
        error!("shutdown deadline exceeded");
        return Err(io::Error::new(io::ErrorKind::TimedOut, "shutdown deadline exceeded").into());
    }
    let release_result = state.lease.release().await;
    state.database.close().await;
    release_result
        .map_err(|error| io::Error::other(format!("durable node release failed: {error}")))?;
    info!("shutdown complete");

    if let Some(result) = server_result {
        result?;
    }
    Ok(())
}

async fn shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}
