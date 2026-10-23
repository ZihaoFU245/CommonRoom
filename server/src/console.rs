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
            let result = match (parts[0], provision) {
                ("/help", _) if parts.len() == 1 => Ok((commands::help(true, true), None)),
                ("/configs", _) if parts.len() == 1 => {
                    app.engine_read().map_err(String::from).and_then(|engine| {
                        app.config
                            .display_with_history(engine.history_store().stats()?)
                            .map(|reply| (reply, None))
                    })
                }
                ("/configs", _) => Err("Usage: /configs".into()),
                (_, Some(account)) => hash_password(account.password)
                    .map_err(String::from)
                    .and_then(|hash| {
                        let mut engine = app.engine()?;
                        let reply =
                            engine.provision(account.name, hash, account.admin, account.reset)?;
                        Ok((reply, Some(Change::Metadata(engine.metadata_change()))))
                    }),
                _ => app.engine().map_err(String::from).and_then(|mut engine| {
                    engine
                        .execute_command(None, None, command, false)
                        .map(|outcome| (outcome.reply, Change::command(outcome.change)))
                        .map_err(String::from)
                }),
            };
            match result {
                Ok((reply, change)) => {
                    if parts.get(0..3) == Some(&["/bot", "token", "new"]) {
                        // Explicit secret issuance goes to the operator, never the tracing log.
                        use std::io::Write;
                        if let Err(error) = writeln!(io::stdout().lock(), "{reply}") {
                            tracing::error!(error = %error, "Could not display credential");
                        }
                    } else {
                        tracing::info!("{reply}");
                    }
                    if let Some(change) = change {
                        let _ = app.changes.send(change);
                    }
                }
                Err(error) => tracing::warn!("{error}"),
            }
        }
        tracing::info!("Console stdin closed; web server remains running.");
    });
}
