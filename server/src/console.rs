use crate::{
    commands,
    engine::{hash_password, sudo_command},
    web::{App, Change},
};
use std::io::{self, BufRead};

pub fn start(app: App) {
    std::thread::spawn(move || {
        tracing::info!("Console ready. /help lists commands. Console input is never logged.");
        for line in io::stdin().lock().lines() {
            let line = match line {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(error = %e, "Console read failed");
                    break;
                }
            };
            let command = match sudo_command(&line) {
                Ok(command) => command.unwrap_or(&line),
                Err(error) => {
                    tracing::warn!("{error}");
                    continue;
                }
            };
            let parts: Vec<_> = command.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }
            let provision = match commands::provision_command(&parts) {
                Ok(provision) => provision,
                Err(error) => {
                    tracing::warn!("{error}");
                    continue;
                }
            };
            let before = match app.engine() {
                Ok(engine) => engine.revision,
                Err(error) => {
                    tracing::error!("{error}");
                    break;
                }
            };
            let result = match (parts[0], provision) {
                ("/help", _) if parts.len() == 1 => Ok(commands::help(true, true)),
                ("/configs", _) if parts.len() == 1 => app.engine_read().and_then(|engine| {
                    app.config
                        .display_with_history(engine.history_store.stats()?)
                }),
                ("/configs", _) => Err("Usage: /configs".into()),
                (_, Some(account)) => hash_password(account.password).and_then(|hash| {
                    app.engine()?
                        .provision(account.name, hash, account.admin, account.reset)
                }),
                _ => app
                    .engine()
                    .and_then(|mut engine| engine.execute(None, None, command)),
            };
            match result {
                Ok(reply) => {
                    tracing::info!("{reply}");
                    if app.engine().is_ok_and(|engine| engine.revision != before) {
                        let _ = app.changes.send(Change::All);
                    }
                }
                Err(error) => tracing::warn!("{error}"),
            }
        }
        tracing::info!("Console stdin closed; web server remains running.");
    });
}
