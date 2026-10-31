use serde::Serialize;

pub const NEW_USER_USAGE: &str = "/user new name password [user|admin]";
pub const NEW_USAGE: &str = "/room new #name | /user new name password [user|admin]";
pub const RM_USAGE: &str =
    "/room delete #name | /user delete name | /revoke user group [#room|@global]";
pub const LS_USAGE: &str = "/room ls | /user ls [scope] | /permissions @groups";
pub const RENAME_USAGE: &str = "/room rename #name new-name | /user rename [who] new-name";

/// Explicit room targets use #; an omitted optional scope uses the current view.
pub fn validate_room_arguments(input: &str) -> Result<(), String> {
    let parts = input.split_whitespace().collect::<Vec<_>>();
    let index = match parts.as_slice() {
        ["/bot", "ls", ..] => Some(2),
        ["/bot", "new" | "info", ..] => Some(3),
        ["/room", "join" | "leave" | "delete" | "rename", ..] => Some(2),
        ["/room", "add" | "kick" | "owner", ..] => Some(3),
        ["/join" | "/leave" | "/members", ..] => Some(1),
        ["/add" | "/kick" | "/owner" | "/clean", ..] => Some(2),
        _ => None,
    };
    if let Some(value) = index.and_then(|index| parts.get(index))
        && !value.starts_with('#')
        && !value.starts_with('@')
    {
        return Err("Room scopes require #name.".into());
    }
    Ok(())
}

/// Route object commands to the existing domain operations; authorization uses the family.
pub fn route(input: &str) -> String {
    let parts: Vec<_> = input.split_whitespace().collect();
    let (family, operation) = match parts.as_slice() {
        [family @ ("/user" | "/room"), operation, ..] => (*family, *operation),
        _ => return input.to_owned(),
    };
    let room_index = match (family, operation) {
        ("/room", "new" | "delete" | "rename" | "join" | "leave") => Some(2),
        ("/room", "add" | "kick" | "owner") => Some(3),
        _ => None,
    };
    let tail = parts
        .iter()
        .enumerate()
        .skip(2)
        .map(|(index, value)| {
            if Some(index) == room_index {
                value.strip_prefix('#').unwrap_or(value)
            } else {
                value
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    match (family, operation) {
        ("/user", "new") => format!("/new user {tail}"),
        ("/user", "delete") => format!("/rm user {tail}"),
        ("/user", "rename") if (3..=4).contains(&parts.len()) => format!("/rename {tail}"),
        ("/user", "reset") if parts.len() == 5 && parts[3] == "password" => {
            format!("/reset {} {}", parts[2], parts[4])
        }
        ("/user", "disable" | "enable" | "passwd") => format!("/{operation} {tail}"),
        ("/room", "new") => format!("/new room {tail}"),
        ("/room", "delete") => format!("/rm room {tail}"),
        ("/room", "rename") => format!("/rename room {tail}"),
        ("/room", "join" | "leave" | "add" | "kick" | "owner") => format!("/{operation} {tail}"),
        _ => input.to_owned(),
    }
}

/// Storage/transport routing only. Domain handlers remain the authority for
/// argument validation and scoped permissions. All adapters share this classifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Execution {
    Read,
    Metadata,
    Message,
    Cleanup,
    Credential,
    Unknown,
}
pub(crate) fn execution(input: &str) -> Execution {
    if !input.trim_start().starts_with('/') {
        return Execution::Message;
    }
    let routed = route(input);
    let parts: Vec<_> = routed.split_whitespace().collect();
    match parts.as_slice() {
        ["/user", "ls" | "get", ..]
        | ["/room", "ls"]
        | ["/bot", "ls" | "info", ..]
        | ["/bot", "token", "ls", ..]
        | [
            "/system" | "/permissions" | "/debug" | "/help" | "/man" | "/whoami" | "/console"
            | "/ls" | "/members" | "/history",
            ..,
        ] => Execution::Read,
        ["/clean", ..] if parts.last() == Some(&"--dry-run") => Execution::Read,
        ["/clean", ..] => Execution::Cleanup,
        ["/tell" | "/reply" | "/edit" | "/react" | "/retract", ..] => Execution::Message,
        ["/passwd" | "/reset", ..] | ["/new", "user", ..] => Execution::Credential,
        [
            "/enable" | "/disable" | "/grant" | "/revoke" | "/add" | "/kick" | "/join" | "/leave"
            | "/owner",
            ..,
        ]
        | ["/new", "room", ..]
        | ["/rm", "room" | "group" | "user", ..]
        | ["/user", "set" | "reset", ..]
        | ["/rename", ..]
        | ["/bot", ..] => Execution::Metadata,
        _ => Execution::Unknown,
    }
}

#[derive(Clone, Serialize)]
pub struct Command {
    pub name: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
    pub requirements: &'static str,
    pub section: &'static str,
    pub admin: bool,
    pub console: bool,
    /// Wrapped commands available through sudo, independent of direct grants.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<Command>,
}

pub fn available(admin: bool, console: bool) -> &'static [Command] {
    static CATALOGUES: std::sync::LazyLock<[Vec<Command>; 4]> = std::sync::LazyLock::new(|| {
        [
            catalogue(false, false),
            catalogue(false, true),
            catalogue(true, false),
            catalogue(true, true),
        ]
    });
    &CATALOGUES[usize::from(admin) * 2 + usize::from(console)]
}
fn catalogue(admin: bool, console: bool) -> Vec<Command> {
    let definitions = [
        (
            "Account",
            "/user",
            "/user ls [scope] | get who [key] [scope] | set who key value [scope] | rename [who] name | new name password [user|admin] | delete who | disable who | enable who | passwd old new | reset who key|@all [scope] | reset who password new-password",
            "List users, inspect or update metadata, and manage accounts",
            false,
        ),
        (
            "Rooms",
            "/room",
            "/room ls | new #name | delete #name | rename #name new-name | join #name | leave [#name] | add user [#room] | kick user [#room] | owner user [#room]",
            "List and manage rooms and membership",
            false,
        ),
        (
            "Server",
            "/bot",
            "/bot ls [#room|@global] | new <bot name> [#room|@global] | info <bot name> [#room|@global] | owner <bot name> <user name> | rm <bot name> @global | token new <bot name> <label> | token ls <bot name> | token rm <credential id> | token rm <bot name> <credential id>",
            "Manage bot accounts and credentials",
            true,
        ),
        (
            "General",
            "/sudo",
            "/sudo /command [arguments]",
            "Run one command with superuser authority",
            true,
        ),
        (
            "General",
            "/permissions",
            "/permissions [scope]",
            "Inspect effective grants",
            false,
        ),
        (
            "General",
            "/debug",
            "/debug on|off",
            "Toggle message details in this browser tab",
            true,
        ),
        (
            "General",
            "/help",
            "/help",
            "List commands available here; /man command explains usage",
            false,
        ),
        (
            "General",
            "/man",
            "/man [command|topic]",
            "Read command and grant-system manuals",
            false,
        ),
        (
            "General",
            "/whoami",
            "/whoami",
            "Show your name and global group label",
            false,
        ),
        (
            "General",
            "/console",
            "/console",
            "Open the Command view in this browser tab",
            false,
        ),
        (
            "Account",
            "/grant",
            "/grant user|@all group [#room|@global] | /grant user|@all scope permission",
            "Assign a group or add a scoped permission; /man grant explains",
            true,
        ),
        (
            "Account",
            "/revoke",
            "/revoke user|@all group [#room|@global] | /revoke user|@all scope permission",
            "Remove a group assignment or direct permission; /man revoke explains",
            true,
        ),
        (
            "Messages",
            "/tell",
            "/tell user message",
            "Send a private message",
            false,
        ),
        (
            "Messages",
            "/edit",
            "/edit message-id message",
            "Edit a retained message; requires own/any-message permission",
            false,
        ),
        (
            "Messages",
            "/retract",
            "/retract message-id",
            "Retract a message for everyone; requires own/any-message permission",
            false,
        ),
        (
            "Messages",
            "/react",
            "/react message-id reaction",
            "Toggle an emoji or text reaction",
            false,
        ),
        (
            "Messages",
            "/reply",
            "/reply message-id message",
            "Reply to a retained message",
            false,
        ),
        (
            "Messages",
            "/history",
            "/history [start end] | /history #room",
            "Read a sequence range or the latest 100 messages from #room",
            false,
        ),
        (
            "Server",
            "/system",
            "/system configs | metrics [--txt]",
            "Inspect server configuration and lifetime metrics",
            true,
        ),
        (
            "Server",
            "/clean",
            "/clean age scope [--dry-run]",
            "Delete or preview old messages in an explicit room or private scope",
            true,
        ),
    ];
    let mut result: Vec<_> = definitions
        .into_iter()
        .filter(|(_, name, _, _, privileged)| {
            (!privileged || admin)
                && (!console
                    || ![
                        "/passwd", "/join", "/leave", "/tell", "/history", "/react", "/reply",
                        "/debug", "/console",
                    ]
                    .contains(name))
        })
        .map(|(section, name, usage, description, admin)| Command {
            name,
            usage,
            description: if console && name == "/retract" {
                "Delete any retained message by ID"
            } else {
                description
            },
            section,
            requirements: requirements(name),
            admin,
            console: false,
            subcommands: Vec::new(),
        })
        .collect();
    if !console {
        result.push(Command {
            name: "/clear",
            usage: "/clear",
            description: "Clear this view locally; refresh restores messages",
            section: "General",
            requirements: requirements("/clear"),
            admin: false,
            console: false,
            subcommands: Vec::new(),
        });
        result.push(Command {
            name: "/logout",
            usage: "/logout",
            description: "Sign out",
            section: "Account",
            requirements: requirements("/logout"),
            admin: false,
            console: false,
            subcommands: Vec::new(),
        });
    }
    result
}

/// Documentation only. Domain handlers remain the authority for each operation.
pub fn requirements(command: &str) -> &'static str {
    match command {
        "/user" => {
            "Account/member listing, account lifecycle or metadata permissions on the target; color/displayname writes follow root ownership or their room own/any grants; other metadata writes are su-only; metadata values never grant access"
        }
        "/room" => "Room discovery, creation or management permissions on the target room",
        "/sudo" => "x:command.sudo and /sudo at @global; temporary superuser authority",
        "/new" => {
            "w:room.create or x:account.create at @global; x:group.admin.assign for admin accounts"
        }
        "/ls" => "room discovery/read grants; r:account.list for accounts and group members",
        "/rm" => "x:room.delete, x:account.delete, or group-management authority on the target",
        "/bot" => "Lifecycle/credentials: global admin or su",
        "/system" => "global /system command grant (included for su)",
        "/rename" => {
            "x:room.rename on the target room; w:account.rename.own on your account or x:account.rename.any on the target account; only su can manage su accounts"
        }
        "/passwd" => "w:account.password.own on your account",
        "/reset" => {
            "x:account.password.reset on the target account; only su can manage su accounts"
        }
        "/disable" => "x:account.disable on the target account; only su can manage su accounts",
        "/enable" => "x:account.enable on the target account; only su can manage su accounts",
        "/join" => "r:message.read + x:room.join on the target room",
        "/members" => "r:member.list on the target conversation",
        "/history" => "r:message.read on each target conversation",
        "/reply" => "r:message.read + w:message.create on the message's conversation",
        "/react" => "r:message.read + w:message.react on the message's conversation",
        "/edit" => {
            "r:message.read + w:message.edit.own (own messages) or w:message.edit.any on the conversation"
        }
        "/retract" => {
            "r:message.read + w:message.retract.own (own messages) or w:message.retract.any on the conversation"
        }
        "/tell" => {
            "w:message.create on the private pair; also w:private.create at @global for a new pair"
        }
        "/add" => "x:member.add on the room + authority to delegate its full participant bundle",
        "/kick" => "x:member.remove on the room; owners must transfer ownership first",
        "/owner" => "x:room.owner.transfer on the target room",
        "/clean" => {
            "x:history.clean on every target; history.clean_min_age metadata limits cleanup; su bypasses both"
        }
        "/debug" => "r:message.metadata on readable conversations",
        "/grant" | "/revoke" => {
            "x:policy.change for individual/room grants; x:group.admin.assign for global user/admin groups; su-only and delegation rules also apply"
        }
        "/permissions" => {
            "r:message.read or r:policy.read for a resource; r:policy.read for grant details; su for @audit"
        }
        "/leave" => "your own room access; transfer ownership before leaving",
        _ => "",
    }
}

pub fn help(admin: bool, console: bool) -> String {
    help_for(available(admin, console))
}

/// Expand alternative forms into complete invocations without splitting argument
/// choices such as [user|admin] or user|@all.
pub fn usage_forms(command: &Command) -> Vec<String> {
    command
        .usage
        .split(" | ")
        .map(|form| {
            if form.starts_with('/') {
                form.to_owned()
            } else {
                format!("{} {form}", command.name)
            }
        })
        .collect()
}

fn form_description(command: &Command, usage: &str) -> &'static str {
    let parts = usage.split_whitespace().skip(1).take(2).collect::<Vec<_>>();
    match (command.name, parts.as_slice()) {
        ("/system", ["configs"]) => "Show active configuration in a Markdown JSON block",
        ("/system", ["metrics", ..]) => {
            "Show lifetime cache and pool counters; --txt returns a shareable text code block"
        }
        ("/user", ["ls", ..]) => "List accounts globally or members of a room",
        ("/user", ["get", ..]) => "Read one user's metadata, or use @all for the scope",
        ("/user", ["set", ..]) => {
            "Set scoped metadata; color uses ownership/room grants, other keys require su"
        }
        ("/user", ["rename", ..]) => "Rename yourself or an authorized account",
        ("/user", ["new", ..]) => "Create an account; the default group is user",
        ("/user", ["delete", ..]) => "Delete an account and its private conversations",
        ("/user", ["disable", ..]) => {
            "Disable an account, revoke sessions and room access, and transfer owned rooms to su"
        }
        ("/user", ["enable", ..]) => {
            "Enable login without restoring previous room access or ownership"
        }
        ("/user", ["passwd", ..]) => "Change your password and sign out other sessions",
        ("/user", ["reset", ..]) => {
            "Reset scoped metadata; color uses ownership/room grants, other keys require su; or reset a password"
        }
        ("/room", ["ls", ..]) => "List rooms you can discover or read",
        ("/room", ["new", ..]) => "Create an invitation-only room",
        ("/room", ["delete", ..]) => "Delete a room and its messages",
        ("/room", ["rename", ..]) => "Rename a room while preserving its identity and history",
        ("/room", ["join", ..]) => "Open an authorized room",
        ("/room", ["leave", ..]) => "Leave a room after transferring ownership if needed",
        ("/room", ["add", ..]) => "Invite a user to a room",
        ("/room", ["kick", ..]) => "Remove a user's room membership and scoped access",
        ("/room", ["owner", ..]) => "Transfer room ownership",
        ("/bot", ["new", ..]) => "Create a bot account in the selected scope",
        ("/bot", ["ls", ..]) => "List bot accounts in a scope",
        ("/bot", ["info", ..]) => "Show bot ownership, status and permissions",
        ("/bot", ["owner", ..]) => "Assign the bot owner",
        ("/bot", ["rm", ..]) => "Delete a bot account and its credentials",
        ("/bot", ["token", "new"]) => "Issue an issuer-only bot token, shown once",
        ("/bot", ["token", "ls"]) => "List issuer/worker credentials, scopes and expiry",
        ("/bot", ["token", "rm"]) => "Revoke a credential",
        ("/grant", [_, "group", ..]) => "Assign a predefined permission bundle",
        ("/grant", _) => "Add a scoped action or command grant",
        ("/revoke", [_, "group", ..]) => "Remove a predefined group assignment",
        ("/revoke", _) => "Remove a direct scoped permission",
        _ => command.description,
    }
}

pub fn help_for(commands: &[Command]) -> String {
    let sections = ["General", "Account", "Rooms", "Messages", "Server"]
        .into_iter()
        .filter_map(|section| {
            let entries = commands
                .iter()
                .filter(|c| c.section == section)
                .map(|command| {
                    let forms = usage_forms(command);
                    if forms.len() == 1 {
                        format!("- `{}` — {}", forms[0], command.description)
                    } else {
                        let lines = forms
                            .iter()
                            .map(|usage| {
                                format!("- `{usage}` — {}", form_description(command, usage))
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        format!(
                            "### `{}`\n\n{}\n\n{lines}",
                            command.name, command.description
                        )
                    }
                })
                .collect::<Vec<_>>();
            (!entries.is_empty()).then(|| format!("## {section}\n\n{}", entries.join("\n\n")))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "# Commands\n\nAvailable in this context. Use `/man command` for examples, scopes and required permissions.\n\n{sections}"
    )
}

/// Shared syntax for the console and asynchronous web password jobs.
#[derive(Clone, Copy)]
pub struct ProvisionCommand<'a> {
    pub name: &'a str,
    pub password: &'a str,
    pub admin: bool,
    pub reset: bool,
}

pub fn provision_command<'a>(parts: &[&'a str]) -> Result<Option<ProvisionCommand<'a>>, String> {
    let (name, password, role, reset) = match parts {
        ["/new", "user", name, password] | ["/user", "new", name, password] => {
            (*name, *password, "user", false)
        }
        ["/new", "user", name, password, role] | ["/user", "new", name, password, role] => {
            (*name, *password, *role, false)
        }
        ["/reset", name, password] | ["/user", "reset", name, "password", password] => {
            (*name, *password, "user", true)
        }
        ["/new", "user", ..] | ["/user", "new", ..] => {
            return Err(format!("Usage: {NEW_USER_USAGE}"));
        }
        ["/reset", ..] | ["/user", "reset", _, "password", ..] => {
            return Err("Usage: /user reset who password new-password".into());
        }
        _ => return Ok(None),
    };
    if reset && name == "@all" {
        return Err("Password reset requires one account.".into());
    }
    if !["user", "admin"].contains(&role) {
        return Err("Role must be admin or user.".into());
    }
    Ok(Some(ProvisionCommand {
        name,
        password,
        admin: role == "admin",
        reset,
    }))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod output_tests {
    use super::*;

    #[test]
    fn reset_password_selector_is_explicit_and_metadata_stays_on_the_object_route() {
        let account = provision_command(&["/user", "reset", "bob", "password", "new-secret"])
            .expect("valid syntax")
            .expect("password operation");
        assert!(account.reset);
        assert_eq!(account.password, "new-secret");
        assert_eq!(
            route("/user reset bob password new-secret"),
            "/reset bob new-secret"
        );
        for parts in [
            vec!["/user", "reset", "bob", "@all"],
            vec!["/user", "reset", "bob", "media.max_age", "#a"],
        ] {
            assert!(
                provision_command(&parts)
                    .expect("metadata syntax")
                    .is_none()
            );
            let input = parts.join(" ");
            assert_eq!(route(&input), input);
        }
        assert!(provision_command(&["/user", "reset", "bob", "password"]).is_err());
        assert!(provision_command(&["/user", "reset", "@all", "password", "new-secret"]).is_err());
    }

    #[test]
    fn help_prints_full_subcommands_as_markdown_instead_of_pipe_chains() {
        let catalogue = available(true, false);
        let text = help_for(catalogue);
        assert!(text.starts_with("# Commands\n\n"));
        for name in ["/user", "/room", "/bot", "/grant", "/revoke"] {
            let command = catalogue
                .iter()
                .find(|c| c.name == name)
                .expect("registered command");
            assert!(text.contains(&format!("### `{name}`\n\n")));
            for usage in usage_forms(command) {
                assert!(usage.starts_with(&format!("{name} ")));
                assert!(text.contains(&format!("- `{usage}` — ")));
                assert!(!usage.contains(" | "));
            }
        }
        assert!(text.contains("/user new name password [user|admin]"));
        assert!(text.contains("/bot token rm <credential id>"));
        assert!(text.contains("/bot token rm <bot name> <credential id>"));
        assert!(!text.contains("[General]"));
    }

    #[test]
    fn help_keeps_authorized_filtering_and_manual_usage_has_one_form_per_line() {
        let catalogue = available(true, false);
        let only = catalogue
            .iter()
            .filter(|c| c.name == "/room")
            .cloned()
            .collect::<Vec<_>>();
        let text = help_for(&only);
        assert!(text.contains("/room rename #name new-name"));
        assert!(!text.contains("/user set"));
        let manual = crate::manual::manual(Some("user")).expect("user manual");
        assert!(manual.contains("```text\n/user ls [scope]\n/user get who [key] [scope]\n/user set who key value [scope]"));
        assert!(!manual.contains(" | get who"));
    }
}
