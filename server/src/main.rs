mod bots;
mod commands;
mod config;
mod console;
mod engine;
mod manual;
mod media;
mod web;

use config::Config;
use engine::Engine;
use std::{
    io,
    path::Path,
    sync::{Arc, RwLock},
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
    // Explicit development setup exits before opening any chat data.
    if std::env::args().nth(1).as_deref() == Some("--setup-local-s3") {
        media::setup_local_s3().await?;
        return Ok(());
    }
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
    engine.media_config = config.media.clone();
    if engine.data.users.is_empty() {
        tracing::info!("No accounts yet. Console: /new user alice a-long-password admin");
    }
    let media = media::MediaStorage::from_config(config.production, config.s3.as_ref())?;
    if media.as_ref().is_some_and(|storage| {
        config
            .origins
            .iter()
            .any(|origin| *origin == storage.public_origin)
    }) {
        return Err("Media storage must use a separate origin from the chat application.".into());
    }
    if media.is_some() {
        engine.prepare_media_cleanup()?;
    }
    let (changes, _) = broadcast::channel(16);
    let app = App {
        media,
        engine: Arc::new(RwLock::new(engine)),
        attempts: Default::default(),
        changes,
        config: Arc::new(config),
        connections: Arc::new(tokio::sync::Semaphore::new(128)),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        shutdown: Default::default(),
    };
    let listener = tokio::net::TcpListener::bind(&app.config.bind).await?;
    console::start(app.clone());
    web::media::start_cleanup(app.clone());
    tracing::info!(address = %listener.local_addr()?, production = app.config.production, "Chat server ready");
    axum::serve(
        listener,
        web::router(app.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown({
        let app = app.clone();
        async move {
            if let Err(error) = shutdown_signal().await {
                tracing::error!(error = %error, "Shutdown signal handler failed");
            }
            app.stopping
                .store(true, std::sync::atomic::Ordering::SeqCst);
            app.shutdown.notify_one();
            let _ = app.changes.send(web::Change::All);
            tracing::info!("Shutting down");
        }
    })
    .await?;
    app.engine()?.checkpoint()?;
    Ok(())
}
async fn shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => { result?; }, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
    }
    Ok(())
}
