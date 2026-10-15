use serde::Serialize;

pub const NEW_USER_USAGE: &str = "/new user name password [user|admin]";
pub const NEW_USAGE: &str = "/new room name | /new user name password [user|admin]";
pub const RM_USAGE: &str = "/rm room name | /rm user name | /rm group user group [room]";
pub const LS_USAGE: &str = "/ls [room|user|group|@user|@admin|@su]";
pub const RENAME_USAGE: &str =
    "/rename room name new-name | /rename user name new-name | /rename [user] new-name";

#[derive(Clone, Serialize)]
pub struct Command {
    pub name: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
    pub requirements: &'static str,
    pub section: &'static str,
    pub admin: bool,
    pub console: bool,
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
            "Server",
            "/bot",
            "/bot new name [scope] | ls [scope] | info name [scope] | owner name user | rm name @global | token new name label | token ls name | token rm name id",
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
            "Rooms",
            "/owner",
            "/owner user [room]",
            "Transfer room ownership",
            true,
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
            "General",
            "/rename",
            RENAME_USAGE,
            "Rename a room, yourself or an authorized account",
            false,
        ),
        (
            "Account",
            "/passwd",
            "/passwd old new",
            "Change your password; sign out other sessions",
            false,
        ),
        (
            "Account",
            "/reset",
            "/reset user password",
            "Reset a password and revoke sessions",
            true,
        ),
        (
            "Account",
            "/disable",
            "/disable user",
            "Disable an account and revoke sessions",
            true,
        ),
        (
            "Account",
            "/enable",
            "/enable user",
            "Enable an account",
            true,
        ),
        (
            "Account",
            "/grant",
            "/grant user group [room] | /grant user scope permission [minimum-age]",
            "Assign a group or add a scoped permission; /man grant explains",
            true,
        ),
        (
            "Account",
            "/revoke",
            "/revoke user group [room] | /revoke user scope permission",
            "Remove a group assignment or direct permission; /man revoke explains",
            true,
        ),
        (
            "Rooms",
            "/members",
            "/members [room]",
            "List conversation participants",
            false,
        ),
        (
            "Rooms",
            "/join",
            "/join room",
            "Open an authorized room; su and sudo also save participant membership",
            false,
        ),
        ("Rooms", "/leave", "/leave [room]", "Leave a room", false),
        (
            "General",
            "/new",
            NEW_USAGE,
            "Create an invitation-only room or account (default group: user)",
            true,
        ),
        (
            "Rooms",
            "/add",
            "/add user [room]",
            "Invite a user to a room",
            true,
        ),
        (
            "Rooms",
            "/kick",
            "/kick user [room]",
            "Remove a user from a room",
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
            "/history [start end] or /history #room",
            "Read a sequence range or the latest 100 messages from #room",
            false,
        ),
        (
            "Server",
            "/configs",
            "/configs",
            "Show active configuration; changes require restart",
            true,
        ),
        (
            "Server",
            "/clean",
            "/clean age [room|@private|@all]",
            "Delete messages older than an age, e.g. 7d or 24h",
            true,
        ),
        (
            "General",
            "/ls",
            LS_USAGE,
            "List rooms (default), accounts, predefined groups, or global group members",
            false,
        ),
        (
            "General",
            "/rm",
            RM_USAGE,
            "Delete a room/account or remove a group assignment; web confirms first",
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
        });
        result.push(Command {
            name: "/logout",
            usage: "/logout",
            description: "Sign out",
            section: "Account",
            requirements: requirements("/logout"),
            admin: false,
            console: false,
        });
    }
    result
}

/// Documentation only. Domain handlers remain the authority for each operation.
pub fn requirements(command: &str) -> &'static str {
    match command {
        "/sudo" => "x:command.sudo and /sudo at @global; temporary superuser authority",
        "/new" => {
            "w:room.create or x:account.create at @global; x:group.admin.assign for admin accounts"
        }
        "/ls" => "room discovery/read grants; r:account.list for accounts and group members",
        "/rm" => "x:room.delete, x:account.delete, or group-management authority on the target",
        "/bot" => "Lifecycle/credentials: global admin or su",
        "/configs" => "r:server.config at @global",
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
        "/clean" => "x:history.clean on every target; minimum-age constraints may apply",
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

pub fn help_for(commands: &[Command]) -> String {
    ["General", "Account", "Rooms", "Messages", "Server"]
        .into_iter()
        .filter_map(|section| {
            let lines: Vec<_> = commands
                .iter()
                .filter(|c| c.section == section)
                .map(|c| format!("{} — {}", c.usage, c.description))
                .collect();
            (!lines.is_empty()).then(|| format!("[{section}]\n{}", lines.join("\n")))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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
        ["/new", "user", name, password] => (*name, *password, "user", false),
        ["/new", "user", name, password, role] => (*name, *password, *role, false),
        ["/reset", name, password] => (*name, *password, "user", true),
        ["/new", "user", ..] => return Err(format!("Usage: {NEW_USER_USAGE}")),
        ["/reset", ..] => return Err("Usage: /reset name password".into()),
        _ => return Ok(None),
    };
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
