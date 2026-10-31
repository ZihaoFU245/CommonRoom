//! Text limits count Unicode scalar values; storage/header limits count UTF-8 bytes.
pub(crate) const BROWSER_BODY_BYTES: usize = 4096;
pub(crate) const BOT_BODY_BYTES: usize = 32768;

pub(crate) const MAX_MESSAGE_CHARS: usize = 4_000;
pub(crate) const MAX_COMMAND_CHARS: usize = 4_100;
pub(crate) const MAX_PASSWORD_BYTES: usize = 128;
pub(crate) const MAX_REACTION_CHARS: usize = 128;
pub(crate) const MESSAGE_TOO_LONG: &str = "Messages support at most 4000 characters.";
pub(crate) const COMMAND_TOO_LONG: &str = "Commands support at most 4100 characters.";

pub(crate) fn validate_input(input: &str) -> Result<(), String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter a message or a command.".into());
    }
    let (limit, error) = if input.starts_with('/') {
        (MAX_COMMAND_CHARS, COMMAND_TOO_LONG)
    } else {
        (MAX_MESSAGE_CHARS, MESSAGE_TOO_LONG)
    };
    if input.chars().take(limit + 1).count() > limit {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn unicode_messages_and_commands_have_distinct_consistent_boundaries() {
        assert!(validate_input(&"界".repeat(MAX_MESSAGE_CHARS)).is_ok());
        assert_eq!(
            validate_input(&"界".repeat(MAX_MESSAGE_CHARS + 1)).unwrap_err(),
            MESSAGE_TOO_LONG
        );
        let command = format!("/{}", "😀".repeat(MAX_COMMAND_CHARS - 1));
        assert!(validate_input(&command).is_ok());
        assert_eq!(
            validate_input(&(command + "😀")).unwrap_err(),
            COMMAND_TOO_LONG
        );
        assert!(validate_input(" \n ").is_err());
    }
}
