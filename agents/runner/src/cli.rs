//! Command-line parsing.
//!
//! Hand-written rather than pulled from a crate: the surface is three
//! subcommands and two options, and this keeps the binary's dependency graph to
//! the runtime and JSON layers.

use std::path::PathBuf;

/// Flags shared by every subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgs {
    /// Subcommand: `serve`, `list` or `config`.
    pub command: String,
    /// `--config` value.
    pub config: Option<PathBuf>,
    /// `--data` value.
    pub data: Option<PathBuf>,
    /// `--help` was passed, or no subcommand was given.
    pub help: bool,
}

impl ParsedArgs {
    /// Parse arguments, excluding `argv[0]`.
    ///
    /// Returns a message suitable for stderr when the command line is malformed.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut command: Option<String> = None;
        let mut config = None;
        let mut data = None;
        let mut help = false;

        let mut index = 0;
        while index < args.len() {
            let argument = args[index].as_str();
            match argument {
                "-h" | "--help" => help = true,
                "--config" => {
                    index += 1;
                    let value = args
                        .get(index)
                        .ok_or_else(|| "--config needs a path".to_string())?;
                    config = Some(PathBuf::from(value));
                }
                "--data" => {
                    index += 1;
                    let value = args
                        .get(index)
                        .ok_or_else(|| "--data needs a path".to_string())?;
                    data = Some(PathBuf::from(value));
                }
                other if other.starts_with('-') => {
                    return Err(format!("unknown option `{other}`"));
                }
                other => {
                    if command.is_some() {
                        return Err(format!("unexpected argument `{other}`"));
                    }
                    command = Some(other.to_string());
                }
            }
            index += 1;
        }

        let command = match command {
            Some(command) => command,
            None if help => "help".to_string(),
            None => String::new(),
        };

        Ok(Self {
            command,
            config,
            data,
            help,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test assertions fail the test on purpose.
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> ParsedArgs {
        let owned: Vec<String> = args.iter().map(|value| (*value).to_string()).collect();
        ParsedArgs::parse(&owned).expect("valid command line")
    }

    #[test]
    fn parses_the_serve_command_with_paths() {
        let parsed = parse(&["serve", "--config", "/tmp/c.json", "--data", "/tmp/data"]);
        assert_eq!(parsed.command, "serve");
        assert_eq!(parsed.config, Some(PathBuf::from("/tmp/c.json")));
        assert_eq!(parsed.data, Some(PathBuf::from("/tmp/data")));
        assert!(!parsed.help);
    }

    #[test]
    fn no_command_means_help() {
        let parsed = parse(&[]);
        assert_eq!(parsed.command, "");
        assert!(!parsed.help);
        let parsed = parse(&["--help"]);
        assert_eq!(parsed.command, "help");
        assert!(parsed.help);
    }

    #[test]
    fn options_may_precede_the_command() {
        let parsed = parse(&["--data", "/d", "list"]);
        assert_eq!(parsed.command, "list");
        assert_eq!(parsed.data, Some(PathBuf::from("/d")));
    }

    #[test]
    fn rejects_unknown_options_and_missing_values() {
        let owned = vec!["serve".to_string(), "--nope".to_string()];
        assert!(ParsedArgs::parse(&owned).is_err());
        let owned = vec!["serve".to_string(), "--config".to_string()];
        assert!(ParsedArgs::parse(&owned).is_err());
    }

    #[test]
    fn rejects_a_second_command() {
        let owned = vec!["serve".to_string(), "list".to_string()];
        let error = ParsedArgs::parse(&owned).expect_err("must fail");
        assert!(error.contains("unexpected argument"), "{error}");
    }
}
