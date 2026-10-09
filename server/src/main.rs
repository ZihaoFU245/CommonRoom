mod commands;
mod config;
mod console;
mod engine;
mod web;

use config::Config;
use engine::Engine;
use std::{
    collections::HashMap,
    io,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;
use web::App;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(io::stdout)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "chat=info,tower_http=info".into()),
        )
        .init();
    let data_dir = std::env::var("CHAT_DATA").unwrap_or_else(|_| "data".into());
    std::fs::create_dir_all(&data_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut engine = Engine::open(&Path::new(&data_dir).join("chat.sqlite"))?;
    let config = Config::load(Path::new(&data_dir))?;
    engine.set_limits(config.max_users, config.max_rooms, config.max_messages)?;
    if engine.data.users.is_empty() {
        tracing::info!("No accounts yet. Console: /user alice a-long-password admin");
    }
    let (changes, _) = broadcast::channel(16);
    let app = App {
        engine: Arc::new(Mutex::new(engine)),
        attempts: Arc::new(Mutex::new(HashMap::new())),
        changes,
        config: Arc::new(config),
        connections: Arc::new(tokio::sync::Semaphore::new(128)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let listener = tokio::net::TcpListener::bind(&app.config.bind).await?;
    console::start(app.clone());
    tracing::info!(address = %listener.local_addr()?, production = app.config.production, "Chat server ready");
    axum::serve(
        listener,
        web::router(app.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown({
        let app = app.clone();
        async move {
            shutdown_signal().await;
            app.stopping
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = app.changes.send(web::Change::All);
            tracing::info!("Shutting down");
        }
    })
    .await?;
    app.engine.lock().unwrap().checkpoint()?;
    Ok(())
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Install SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
