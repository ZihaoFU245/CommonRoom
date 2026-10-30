//! Bounded HTTP admission and absolute header deadlines, including direct bot ingress.
use axum::{Extension, Router, extract::ConnectInfo};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use std::{future::Future, io, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::Semaphore};

pub(crate) async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()>,
) -> io::Result<()> {
    serve_with_limits(listener, router, shutdown, 512, Duration::from_secs(15)).await
}
async fn serve_with_limits(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()>,
    connections: usize,
    header_timeout: Duration,
) -> io::Result<()> {
    let permits = Arc::new(Semaphore::new(connections));
    let (stop, _) = tokio::sync::watch::channel(());
    let mut tasks = tokio::task::JoinSet::new();
    tokio::pin!(shutdown);
    let result = loop {
        let accepted = tokio::select! {
            biased;
            _ = &mut shutdown => break Ok(()),
            _ = tasks.join_next(), if !tasks.is_empty() => continue,
            accepted = async {
                // Backpressure before accept bounds tasks and open server sockets.
                let permit = permits.clone().acquire_owned().await
                    .map_err(io::Error::other)?;
                loop {
                    match listener.accept().await {
                        Ok((stream, address)) => return Ok::<_, io::Error>((stream, address, permit)),
                        Err(error) if matches!(error.kind(), io::ErrorKind::ConnectionAborted | io::ErrorKind::Interrupted) => continue,
                        Err(error) => {
                            // Match axum's accept retry behavior: temporary FD
                            // pressure must not terminate an otherwise healthy server.
                            tracing::warn!(%error, "HTTP accept failed; retrying");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            } => accepted,
        };
        let (stream, address, permit) = match accepted {
            Ok(accepted) => accepted,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionAborted | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(error) => break Err(error),
        };
        let service = TowerToHyperService::new(
            router
                .clone()
                .layer(Extension(ConnectInfo(address)))
                .into_service(),
        );
        let mut stopped = stop.subscribe();
        tasks.spawn(async move {
            let _permit = permit;
            let mut builder = hyper::server::conn::http1::Builder::new();
            builder
                .timer(TokioTimer::new())
                .header_read_timeout(header_timeout);
            let connection = builder
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades();
            tokio::pin!(connection);
            let result = tokio::select! {
                result = &mut connection => result,
                _ = stopped.changed() => {
                    connection.as_mut().graceful_shutdown();
                    connection.await
                }
            };
            if let Err(error) = result {
                tracing::debug!(%error, "HTTP connection ended");
            }
            // Upgraded sockets have their own bounded admission and lifecycle.
        });
    };
    let _ = stop.send(());
    while tasks.join_next().await.is_some() {}
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn slow_headers_expire_and_release_connection_capacity() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let router = Router::new().route(
            "/",
            get(
                |ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>| async move {
                    assert!(peer.ip().is_loopback());
                    "healthy"
                },
            ),
        );
        let server = tokio::spawn(serve_with_limits(
            listener,
            router,
            async {
                let _ = stopped.await;
            },
            1,
            Duration::from_millis(100),
        ));
        let mut slow = tokio::net::TcpStream::connect(address).await.unwrap();
        slow.write_all(b"GET / HTTP/1.1\r\nHost: local")
            .await
            .unwrap();
        let mut healthy = tokio::net::TcpStream::connect(address).await.unwrap();
        healthy
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        // The second connection must wait for the first permit to be released.
        let mut response = Vec::new();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                healthy.read_to_end(&mut response)
            )
            .await
            .is_err()
        );
        tokio::time::timeout(Duration::from_secs(2), healthy.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&response).contains("healthy"));
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
