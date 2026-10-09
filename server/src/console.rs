use crate::{
    commands,
    engine::hash_password,
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
            let parts: Vec<_> = line.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }
            let before = app.engine.lock().unwrap().revision;
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
                            app.engine.lock().unwrap().provision(
                                parts[1],
                                hash,
                                parts.get(3) == Some(&"admin"),
                                reset,
                            )
                        })
                    }
                }
                _ => app.engine.lock().unwrap().execute(None, None, &line),
            };
            match result {
                Ok(reply) => {
                    tracing::info!("{reply}");
                    if app.engine.lock().unwrap().revision != before {
                        let _ = app.changes.send(Change::All);
                    }
                }
                Err(error) => tracing::warn!("{error}"),
            }
        }
        tracing::info!("Console stdin closed; web server remains running.");
    });
}
