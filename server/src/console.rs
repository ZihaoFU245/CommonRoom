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
            let before = match app.engine() {
                Ok(engine) => engine.revision,
                Err(error) => {
                    tracing::error!("{error}");
                    break;
                }
            };
            let result = match parts[0] {
                "/help" => Ok(commands::help(true, true)),
                "/configs" if parts.len() == 1 => app.config.display(),
                "/configs" => Err("Usage: /configs".into()),
                "/user" | "/reset" => {
                    let reset = parts[0] == "/reset";
                    if (reset && parts.len() != 3) || (!reset && !(3..=4).contains(&parts.len())) {
                        Err(
                            "Usage: /user name password [admin|user] or /reset name password"
                                .into(),
                        )
                    } else if !reset
                        && parts
                            .get(3)
                            .is_some_and(|role| !["admin", "user"].contains(role))
                    {
                        Err("Role must be admin or user.".into())
                    } else {
                        hash_password(parts[2]).and_then(|hash| {
                            app.engine()?.provision(
                                parts[1],
                                hash,
                                parts.get(3) == Some(&"admin"),
                                reset,
                            )
                        })
                    }
                }
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
