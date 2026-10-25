mod bots;
mod commands;
mod config;
mod console;
mod engine;
mod limits;
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
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!(
            "chat {} (commit {})",
            env!("CARGO_PKG_VERSION"),
            env!("CHAT_COMMIT")
        );
        return Ok(());
    }
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
    if std::env::args().nth(1).as_deref() == Some("--migrate-sqlite") {
        let source = std::env::args_os()
            .nth(2)
            .ok_or("Usage: chat --migrate-sqlite /path/to/chat.sqlite (set CHAT_DATABASE_URL)")?;
        if std::env::args_os().nth(3).is_some() {
            return Err("Unexpected migration arguments".into());
        }
        let destination = std::env::var("CHAT_DATABASE_URL")
            .map_err(|_| "Migration requires CHAT_DATABASE_URL")?;
        let report = tokio::task::spawn_blocking(move || {
            Engine::migrate_sqlite(Path::new(&source), &destination)
        })
        .await??;
        for (table, rows) in report.rows {
            println!("{table}: {rows} verified rows");
        }
        println!(
            "Migration committed. SQLite source preserved. Set database.backend=postgres in config.json and start chat with CHAT_DATABASE_URL set."
        );
        return Ok(());
    }
    let data_dir = std::env::var("CHAT_DATA").unwrap_or_else(|_| "data".into());
    std::fs::create_dir_all(&data_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    // Read the explicit backend without writing; acquire ownership before
    // configuration normalization. A secret URL never selects the backend.
    // Construct either synchronous database driver on a blocking worker.
    let database = Config::read_database(Path::new(&data_dir))?;
    let selected_database = database.clone();
    let database_path = Path::new(&data_dir).join("chat.sqlite");
    let mut engine = tokio::task::spawn_blocking(move || -> Result<Engine, engine::EngineError> {
        match selected_database.backend {
            config::DatabaseBackend::Sqlite => Engine::open(&database_path),
            config::DatabaseBackend::Postgres => {
                let url = std::env::var("CHAT_DATABASE_URL")
                    .ok()
                    .filter(|url| !url.trim().is_empty())
                    .ok_or("database.backend=postgres requires CHAT_DATABASE_URL.")?;
                Engine::open_postgres_in_folder(&url, &database_path)
            }
        }
    })
    .await??;
    let config = Config::load_for_database(Path::new(&data_dir), &database)?;
    let media = media::MediaStorage::from_config(config.production, config.s3.as_ref())?;
    if media.as_ref().is_some_and(|storage| {
        config
            .origins
            .iter()
            .any(|origin| *origin == storage.public_origin)
    }) {
        return Err("Media storage must use a separate origin from the chat application.".into());
    }
    let setup_config = config.clone();
    let media_enabled = media.is_some();
    let engine = tokio::task::spawn_blocking(move || -> Result<Engine, engine::EngineError> {
        engine.set_limits(
            setup_config.max_users,
            setup_config.max_rooms,
            setup_config.max_messages,
        )?;
        engine.configure_media(setup_config.media.clone());
        engine.set_avatar_hosts(setup_config.avatar_hosts.iter().cloned().collect());
        if media_enabled {
            engine.prepare_media_cleanup()?;
        }
        Ok(engine)
    })
    .await??;
    if !engine.has_accounts() {
        tracing::info!("No accounts yet. Console: /new user alice a-long-password admin");
    }
    let (changes, _) = broadcast::channel(256);
    let app = App {
        media,
        engine: Arc::new(RwLock::new(engine)),
        attempts: Default::default(),
        changes,
        config: Arc::new(config),
        admission: Default::default(),
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
            app.shutdown.notify_waiters();
            let _ = app.changes.send(web::Change::Shutdown);
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
