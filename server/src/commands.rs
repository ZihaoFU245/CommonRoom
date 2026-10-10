use serde::Serialize;

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

pub fn available(admin: bool, console: bool) -> Vec<Command> {
    let definitions = [
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
        ("General", "/help", "/help", "List commands", false),
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
            "Show your name and permission",
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
            "/passwd",
            "/passwd old new",
            "Change your password; sign out other sessions",
            false,
        ),
        (
            "Account",
            "/users",
            "/users",
            "List accounts and permissions",
            false,
        ),
        (
            "Account",
            "/user",
            "/user name password [admin|user]",
            "Create an account",
            true,
        ),
        (
            "Account",
            "/deleteuser",
            "/deleteuser user",
            "Permanently delete an account and its private conversations",
            true,
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
        ("Rooms", "/rooms", "/rooms", "List rooms and owners", false),
        (
            "Rooms",
            "/members",
            "/members [room]",
            "List room members",
            false,
        ),
        (
            "Rooms",
            "/join",
            "/join room",
            "Switch rooms; users must be invited",
            false,
        ),
        ("Rooms", "/leave", "/leave [room]", "Leave a room", false),
        ("Rooms", "/new", "/new room", "Create a room", true),
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
            "Rooms",
            "/delete",
            "/delete room",
            "Delete a room and its messages",
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
            "/retract",
            "/retract message-id",
            "Delete your own message for everyone in its conversation",
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
            "/history [count] [user]",
            "Read retained messages (default up to 50)",
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
        "/new" => "w:room.create at @global",
        "/user" => "x:account.create at @global; also x:group.admin.assign when creating an admin",
        "/configs" => "r:server.config at @global",
        "/users" => "r:account.list at @global",
        "/passwd" => "w:account.password.own on your account",
        "/reset" => {
            "x:account.password.reset on the target account; only su can manage su accounts"
        }
        "/disable" => "x:account.disable on the target account; only su can manage su accounts",
        "/enable" => "x:account.enable on the target account; only su can manage su accounts",
        "/deleteuser" => {
            "x:account.delete on the target account; account-protection rules also apply"
        }
        "/join" => "r:message.read + x:room.join on the target room",
        "/members" => "r:member.list on the target conversation",
        "/history" => "r:message.read on each target conversation",
        "/reply" => "r:message.read + w:message.create on the message's conversation",
        "/react" => "r:message.read + w:message.react on the message's conversation",
        "/retract" => {
            "r:message.read + w:message.retract.own (own messages) or w:message.retract.any on the conversation"
        }
        "/tell" => {
            "w:message.create on the private pair; also w:private.create at @global for a new pair"
        }
        "/add" => "x:member.add on the room + authority to delegate its full participant bundle",
        "/kick" => "x:member.remove on the room; owners must transfer ownership first",
        "/delete" => "x:room.delete on the target room",
        "/owner" => "x:room.owner.transfer on the target room",
        "/clean" => "x:history.clean on every target; minimum-age constraints may apply",
        "/debug" => "r:message.metadata on readable conversations",
        "/grant" | "/revoke" => {
            "x:policy.change for individual/room grants; x:group.admin.assign for global user/admin groups; su-only and delegation rules also apply"
        }
        "/permissions" => {
            "r:message.read or r:policy.read for a resource; r:policy.read for grant details; su for @audit"
        }
        "/rooms" => {
            "r:room.discover or r:message.read for each listed room; r:message.read for your own private pairs"
        }
        "/leave" => "your own room access; transfer ownership before leaving",
        _ => "",
    }
}

pub fn help(admin: bool, console: bool) -> String {
    help_for(&available(admin, console))
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
