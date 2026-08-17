use std::{error::Error, io, sync::Arc, time::Duration};

use axum::{Extension, Router};
use hyper::server::conn::http1;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use owlmux_server::{
    build, config::Config, internal::InternalRuntime, runtime::RuntimeTasks, service::ServerState,
};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, oneshot},
    task::JoinSet,
    time::{sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const MAX_PUBLIC_CONNECTIONS: usize = 256;
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECTION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

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
    let internal = InternalRuntime::bind(&config).await?;
    let internal_client = internal.as_ref().map(InternalRuntime::client);
    let state = ServerState::bootstrap(config, internal_client).await?;
    let listener = TcpListener::bind(address).await?;
    let local_address = listener.local_addr()?;
    let tasks = RuntimeTasks::new();
    let cancellation = tasks.cancellation_token();
    let (server_result_tx, server_result_rx) = oneshot::channel();

    info!(address = %local_address, web_dir = %web_dir.display(), server_build_id = build::BUILD_ID, incarnation_id = %state.lease.incarnation_id(), "starting OwlMux server");

    if let Some(internal) = internal {
        let internal_state = state.clone();
        let internal_cancellation = cancellation.clone();
        tasks.spawn(async move {
            internal.serve(internal_state, internal_cancellation).await;
        });
    }
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
        let result = serve_public(listener, app, cancellation).await;
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

async fn serve_public(
    listener: TcpListener,
    app: Router,
    cancellation: CancellationToken,
) -> io::Result<()> {
    let admission = Arc::new(Semaphore::new(MAX_PUBLIC_CONNECTIONS));
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            completed = connections.join_next(), if !connections.is_empty() => {
                if completed.is_some_and(|result| result.is_err()) {
                    warn!("public HTTP connection task failed");
                }
            }
            permit = Arc::clone(&admission).acquire_owned() => {
                let permit = permit.map_err(|_| io::Error::other("public HTTP admission closed"))?;
                let accepted = tokio::select! {
                    () = cancellation.cancelled() => break,
                    accepted = listener.accept() => accepted,
                };
                let (stream, peer) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        drop(permit);
                        warn!(%error, "public HTTP accept failed");
                        sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let service = app.clone().layer(Extension(axum::extract::ConnectInfo(peer)));
                let connection_cancellation = cancellation.clone();
                connections.spawn(async move {
                    let _permit = permit;
                    let mut builder = http1::Builder::new();
                    builder
                        .timer(TokioTimer::new())
                        .header_read_timeout(Some(HEADER_READ_TIMEOUT))
                        .max_buf_size(32 * 1024);
                    let connection = builder
                        .serve_connection(
                            TokioIo::new(stream),
                            TowerToHyperService::new(service),
                        )
                        .with_upgrades();
                    tokio::pin!(connection);
                    tokio::select! {
                        result = &mut connection => {
                            if let Err(error) = result {
                                tracing::debug!(%error, %peer, "public HTTP connection closed");
                            }
                        }
                        () = connection_cancellation.cancelled() => {
                            connection.as_mut().graceful_shutdown();
                            let _ = timeout(CONNECTION_SHUTDOWN_TIMEOUT, &mut connection).await;
                        }
                    }
                });
            }
        }
    }
    while let Some(result) = connections.join_next().await {
        if result.is_err() {
            warn!("public HTTP connection task failed during shutdown");
        }
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
