//! Public documentation; reading a manual never grants permission to execute it.
use crate::{commands, engine::authorization::Action};

pub fn manual(topic: Option<&str>) -> Result<String, String> {
    let topic = topic.unwrap_or("overview").trim_start_matches('/');
    let text = match topic {
        "overview" | "man" => OVERVIEW,
        "grant" => GRANT,
        "revoke" => REVOKE,
        "permissions" => PERMISSIONS,
        "groups" => GROUPS,
        "scopes" => SCOPES,
        "ownership" => OWNERSHIP,
        "console" => CONSOLE,
        "sudo" => SUDO,
        "bot" => BOT,
        _ => {
            let name = format!("/{topic}");
            let command = commands::available(true, false)
                .into_iter()
                .find(|command| command.name == name)
                .ok_or_else(|| format!("No manual for '{topic}'. Use /man to list topics."))?;
            return Ok(format!(
                "# {}\n\n{}\n\n## Usage\n\n```\n{}\n```{}\n\n## Access required\n\n- The command grant must allow the selected conversation and its target.\n- The matching action grants must allow each affected resource.\n\n{}\n\n## See also\n\nUse `/help` to list commands available here, or `/man permissions` to understand grants.",
                command.name,
                command.description,
                command.usage,
                match topic {
                    "ls" => LS,
                    "new" => NEW,
                    "rm" => RM,
                    "retract" => RETRACT,
                    _ => "",
                },
                if command.requirements.is_empty() {
                    "No additional action grant is required.".into()
                } else {
                    format!("Requires: {}.", command.requirements)
                }
            ));
        }
    };
    if topic != "permissions" {
        return Ok(text.into());
    }
    let mut text = text.to_owned();
    for (prefix, title) in [
        ("r:", "Read actions"),
        ("w:", "Write actions"),
        ("x:", "Management actions"),
    ] {
        let actions = Action::ALL
            .iter()
            .map(|action| action.name())
            .filter(|name| name.starts_with(prefix))
            .collect::<Vec<_>>()
            .join("\n");
        text.push_str(&format!("\n\n## {title}\n\n```\n{actions}\n```"));
    }
    text.push_str("\n\n## Command grants");
    let catalog = commands::available(true, false);
    for section in ["General", "Account", "Rooms", "Messages", "Server"] {
        let names = catalog
            .iter()
            .filter(|command| command.section == section)
            .map(|command| format!("`{}`", command.name))
            .collect::<Vec<_>>()
            .join(", ");
        text.push_str(&format!("\n\n### {section}\n\n{names}"));
    }
    Ok(text)
}

const OVERVIEW: &str = r#"# CommonRoom manual

Use `/help` to list commands available in the selected conversation.

## Everyday commands

- `/man ls` — list rooms, accounts, groups, and global group members.
- `/man new` — create an invitation-only room or account.
- `/man rm` — delete a room/account or remove a group assignment.
- `/man alias` — customize command prefixes in the web app.

## Usage

```
/man [command|topic]
```

Command names work with or without a slash: `/man grant` and `/man /grant` are equivalent.

## Access control topics

- `/man grant` — assign a group or add one permission.
- `/man revoke` — remove a group or a direct permission.
- `/man permissions` — understand r/w/x and browse the permission catalog.
- `/man groups` — compare user, admin, and su.
- `/man scopes` — choose where access applies.
- `/man ownership` — create rooms, invite people, and transfer ownership.
- `/man sudo` — run one command with temporary superuser authority.

## Other commands

Use `/man history`, `/man reset`, or another command name for its usage.
`/help` lists commands available in the selected conversation.
`/console` opens the Command view; select a room or use `/join room` to return to a conversation.

Manuals are public documentation. Reading one does not grant permission to run the command."#;

const LS: &str = r#"

## Examples

```
/ls
/ls room
/ls user
/ls group
/ls @admin
```

## What is listed?

- `/ls` and `/ls room` list rooms you can discover or read, plus your readable private conversations.
- `/ls user` lists accounts. Disabled accounts are visible only to su.
- `/ls group` lists the fixed user, admin, and su bundles; use `/man groups` for their permissions.
- `/ls @user`, `/ls @admin`, and `/ls @su` filter explicit global group assignments, not room assignments or individual permissions.

An account can belong to several groups. Its displayed label is its highest global group;
an account listed under @user can therefore display admin. Listing never changes access.
Account listings require `r:account.list` at global scope."#;

const NEW: &str = r#"

## Examples

```
/new room support
/new user bob temporary-password
/new user alice temporary-password admin
```

## Rooms

New rooms are invitation-only. The creator owns the room and receives participant
and management grants. Invite Bob with `/add bob support`.
`/join support` opens a room you can already access; it does not grant membership.

## Accounts

The optional group is user or admin; omitting it means user. Creating an admin
requires `x:group.admin.assign` as well as `x:account.create`.
Assign su separately with `/grant username su`, which only an existing su can run.
Names and passwords are single arguments; spaces are not supported.
Passwords are masked in browser command output. Use `/passwd old new` to change
your own password or `/reset username password` with explicit reset authority.

See `/man ownership` for room management and `/man groups` for group defaults."#;

const RM: &str = r#"

## Examples

```
/rm room support
/rm user bob
/rm group bob user support
/rm group alice admin
```

## Delete a room or account

- Removing a room permanently deletes its messages and scoped access.
- Removing an account revokes its sessions and removes its private conversations for both participants.
- Room messages and reply quotes remain attributed to the deleted account.
- Reusing a deleted name does not inherit old grants, history, sessions, or read positions.

Only su can delete a su account. Ordinary admins cannot delete themselves or
the last active admin. Use `/disable username` for reversible account suspension.

## Remove a group assignment

The group must be user, admin, or su. Without a room, remove its global assignment;
with a room, remove that room's assignment. The predefined bundle itself remains.
Direct grants and other group assignments still apply. `/kick username room`
removes membership and scoped access; `/revoke` also removes individual grants.
The delegation and su restrictions described in `/man revoke` apply.

## Confirmation

The web app asks for confirmation before sending `/rm`, including aliases and
sudo. Stdin executes directly. Room/account deletion cannot be undone;
group access can be granted again by an authorized manager."#;

const RETRACT: &str = r#"

## Behavior

Retract a retained message by its ID. It disappears for every participant,
and retained replies lose their quote preview. Read permission is required;
own-message authority applies to your messages, while any-message authority
also allows retracting other authors' messages.

The web app asks for confirmation, including aliases and sudo. Stdin executes
directly. Retraction cannot be undone. The message's hover action supplies its
ID automatically in the web app."#;

const GRANT: &str = r#"# /grant

Give someone a group of permissions, or one permission in a specific place.
The account and resource must already exist.

## Assign a group

```
/grant username [user|admin|su] [room]
```

- Without a room, the group applies at global scope.
- With a room, user gives participant access; admin adds room management.
- Omitting the group means global admin. Write the group explicitly for clarity.

Give Bob participant access to support:

```
/grant bob user support
```

Give Alice every permission:

```
/grant alice su
```

## Add one permission

```
/grant username scope permission [minimum-age]
```

Use an exact action name or a slash command. For history access, grant both:

```
/grant bob support r:message.read
/grant bob support /history
```

These grants add access. They do not remove write access from another grant or group.
Use `/man scopes` for room, global, account, and private-conversation scope names.

## Let someone invite users

As the owner of support, give Alice participant access, then both invitation grants:

```
/add alice support
/grant alice support x:member.add
/grant alice support /add
```

Alice can now run `/add bob support`. Her global user or admin group does not replace room participant access.
The `/add` command grant alone does not grant `x:member.add`.
Remove these direct grants with `/revoke alice support x:member.add` and `/revoke alice support /add`.

## Who can grant access?

- Only an existing su member, including stdin, can assign su. su is always global.
- Only su can change individual global, account, or private grants, or assign room admin.
- Global admin assignment requires `x:group.admin.assign`.
- Room managers need `x:policy.change` and can delegate only participant permissions they hold.
- The room owner may also grant or revoke `x:member.add` and `/add` within that room, if they hold those permissions.
- Giving the room user group requires authority to delegate its full participant bundle.
- Command grants must allow the selected conversation and target; action grants must allow the target.

## Limit history cleanup

Only `x:history.clean` accepts minimum-age. Use positive durations with s/m/h/d/w units.
This example permits cleanup of messages older than seven days; su must issue these grants:

```
/grant bob support x:history.clean 7d
/grant bob support /clean
```

An unrestricted cleanup grant from elsewhere still applies.

## Check access

`/permissions support` shows your effective access and, for authorized managers, the room's grants.
See `/man permissions` for the catalog and `/man revoke` to remove access."#;

const REVOKE: &str = r#"# /revoke

Remove a group assignment or a direct permission.
Access from other grants and groups still applies.

## Remove a group

```
/revoke username [user|admin|su] [room]
```

Remove Bob's participant bundle from support:

```
/revoke bob user support
```

The exact assignment is removed. Omitting the group means global admin.
Only su can remove su assignments. Ordinary admins cannot remove the last active admin.

## Remove one direct permission

```
/revoke username scope permission
```

Remove a direct send permission and a direct reply command grant:

```
/revoke bob support w:message.create
/revoke bob support /reply
```

This removes the named direct permission at that scope, including its cleanup-age variants.
It does not remove the same permission supplied by a group or a global grant.
The scope-management rules from `/man grant` apply here too.

## Make a participant read-only

Replace Bob's room user bundle with grants to find, open, and read support:

```
/revoke bob user support
/grant bob support r:room.discover
/grant bob support x:room.join
/grant bob support r:message.read
/grant bob support /history
```

Inspect `/permissions support` for other direct, room admin, or global grants that may still allow writes.

## Remove room membership

Revoking grants may leave the person's name in the member list.
Use `/kick` to remove membership and their direct/scoped group grants:

```
/kick bob support
```

Global grants still apply. Revocation takes effect on the next server check and updates connected clients.
Content already received by a client cannot be taken back."#;

const PERMISSIONS: &str = r#"# Permissions

A grant allows one action or command in a particular scope. Without a matching grant, access is denied.

## Three kinds of action

- `r` — read information, such as `r:message.read`.
- `w` — create or change content, such as `w:message.create`.
- `x` — perform a management operation, such as `x:member.add`.

## Commands need grants too

Running `/history` requires both its command grant and `r:message.read` on the conversation.
HTTP history requests check read access independently of command grants.

Grants add together. There are no deny rules, wildcards, or groups nested inside groups.
Global grants apply globally; resource grants apply only to the resource's stable identity.

## Message ownership

- `w:message.retract.own` allows retracting your own messages.
- `w:message.retract.any` allows retracting other authors' messages.
- Both also require read access and the `/retract` command grant.

Authorship uses stable account IDs, so reusing a username does not inherit old messages.

## Inspect access

```
/permissions support
/permissions @groups
```

- `/permissions [scope]` shows your effective actions and command grants.
- `r:policy.read` allows viewing scoped grant assignments.
- `/permissions @groups` lists predefined bundles.
- `/permissions @audit` shows the audit log to su.

See `/man grant`, `/man revoke`, `/man groups`, and `/man scopes`.
The catalogs below list exact permission names."#;

const GROUPS: &str = r#"# Groups

Groups are predefined bundles of grants. Their scope determines where the bundle applies.

## user

- At global scope: account directory, own password and rename, new private chats, and general commands including `/man`.
- In a room: find/read/join, member list, send/reply/react, and retract your own messages.

## admin

- At global scope: user access plus account/room creation, account enable/disable/delete, admin assignment, and configuration inspection.
- In a room: participant access plus room management and policy inspection.
- Server admin does not automatically provide access to other people's new rooms or private messages, or password reset.

## su

- Every permission at every scope, including private inspection and retracting any message.
- Only existing su members can add or remove su members.
- Stdin belongs to su and provides recovery access.

## Private chats and migrated accounts

Private participants receive content grants in their original pair while they have a server group.
Individual direct grants still add access.

Migrated legacy admins may have explicit grants preserving older privileges.
Inspect actual grants before assuming fresh-account defaults.

## Manage groups

```
/permissions @groups
/grant bob user support
/revoke bob user support
```

See `/man grant` and `/man revoke` for authorization rules."#;

const SCOPES: &str = r#"# Scopes

A scope is the place where a grant applies.

## Scope names

- `support` — one room, identified internally by its stable ID.
- `@global` — global access across all resource scopes.
- `@account:alice` — one account, identified by its stable ID.
- `@private:alice:bob` — an existing private pair, using its original participant IDs.

Deleting and recreating an account or room does not restore its old grants.
Resource names and message IDs are identifiers, not credentials.

## Who can change each scope?

- Only su can change global, account, or private grants.
- Room managers can delegate their own participant permissions within their room.
- Room owners can also delegate `x:member.add` and `/add` to let participants invite users.

## Selected conversation and target

The selected conversation is where you invoke a command. An explicit argument can select a different target.
Both places must allow the command, and each affected target must allow the action.
The Command view does not bypass target checks.

Cleanup across multiple conversations checks every target before changing any history.

## Examples

Read one room:

```
/grant bob support r:message.read
/grant bob support /history
```

Reset Alice's password (grants issued by su):

```
/grant bob @account:alice x:account.password.reset
/grant bob @account:alice /reset
```

Inspect an existing private pair (grants issued by su):

```
/grant auditor @private:alice:bob r:message.read
/grant auditor @private:alice:bob /history
```

Command invocation still needs its own grant in the selected context. Use `/man grant` for the full rules."#;

const SUDO: &str = r#"# /sudo

Run one slash command with temporary superuser authority.

## Usage

```
/sudo /command [arguments]
```

## Access required

Both grants must apply at global scope. Only su can grant this authority:

```
/grant bob @global /sudo
/grant bob @global x:command.sudo
```

Neither grant alone permits elevation. User and admin groups do not include sudo;
the su group includes both grants.

## Examples

```
/sudo /new room support
/sudo /reset alice new-password
/sudo /permissions @audit
```

The wrapped command receives all su command and action permissions. It can manage
su accounts and grant permanent privileges, so sudo delegates full superuser trust.
The caller keeps their name and stable identity in messages, room ownership, and
audit entries. Elevation does not itself change groups or persist any grants.
Normal validation, storage rollback, and session checks still apply.

For password commands, both sudo grants are rechecked after hashing. Nested sudo
and ordinary message text are rejected. Browser-local clear and logout complete
in the requesting tab after the server authorizes sudo.

## Revoke

```
/revoke bob @global /sudo
/revoke bob @global x:command.sudo
```

Revoking either grant blocks future sudo commands. Explicit group assignments and
other additive grants still apply."#;

const CONSOLE: &str = r#"# /console

Open the Command view in this browser tab. There is no Command button in the sidebar.

## Usage

```
/console
```

Accounts with no conversations start in this view automatically.
Select a conversation in the sidebar, or use `/join room`, to leave it.
Each view keeps its own command output until the tab is refreshed.

## Access required

The `/console` command grant must allow the selected context. The user group includes it by default.
Opening the Command view does not add permissions. Commands still check their context, targets, and required actions.
This command is available in the web UI only."#;

const OWNERSHIP: &str = r#"# Room ownership

The person who creates a room owns it and receives participant and management grants.

## Create and invite

Creating a room requires `w:room.create` at global scope and the `/new` command grant.
Creation saves the room, stable identity, ownership, and grants together.

```
/new room support
/add bob support
```

New rooms are invitation-only. `/join` opens a room you already have access to; it never creates membership or grants.
Another server admin does not automatically gain access to your new room.

## Owner permissions

- Invite or remove participants.
- Inspect and change participant grants.
- Delegate invitations with both `x:member.add` and `/add`; see `/man grant` for the steps.
- Clean history and delete the room.
- Transfer ownership to another account.

Ownership does not grant account/server management or permission to retract other authors' messages.

## Transfer ownership

```
/owner bob support
```

This transfers ownership and its management grants. Existing participant access remains.
An owner must transfer ownership before leaving or being kicked.

## Recovery

Disabling or deleting an owner returns the room to the console. su has global recovery access.
Use `/permissions support` to inspect the room's grants."#;

const BOT: &str = r#"# /bot

Manage bot accounts, tokens and external runtime settings. `[scope]` and `[key]` are optional.

## Accounts

- `/bot new name [scope]` — create a bot with read/send/reply/react and manual access.
- `/bot ls [scope]` — list bots; a room scope filters by membership.
- `/bot info name [scope]` — show status, connections and permissions.
- `/bot rm name @global` — delete the account and its tokens.

Scope defaults to the current room. Console/private views need an explicit room. Always write `@global` explicitly.

## Tokens

- `/bot token new name label` — issue a token; save the secret shown once.
- `/bot token ls name` — list token IDs and labels.
- `/bot token rm name token-id` — revoke a token and disconnect its sockets.

Tokens follow the account's current grants.

## Remote configuration

- `/bot config set name key value` — ask the connected runtime to apply a setting.
- `/bot config fetch name [key]` — fetch one setting, or all when key is omitted.

Values are strings; quote with JSON syntax when needed. The external runtime validates and saves settings. It must be online; requests time out after 10 seconds. Fetch before retrying an uncertain set.

## Permissions

- Every operation needs `/bot` command access in its context and target.
- Accounts/tokens: global admin, su or stdin; ordinary account/scope permissions also apply. Tokens require `x:bot.credentials`.
- Config fetch: `r:bot.config` on `@account:name`.
- Config set: `w:bot.config` on `@account:name`.
- Config access can be delegated; account-scope grants require su/stdin. Protected su accounts require su.

Use `/grant`, `/revoke`, `/add`, `/kick`, `/disable`, `/enable` and `/rename` as for any user.

## Examples

```
/bot new helper support
/bot token new helper worker
/bot config set helper prompt "Help with Rust questions."
/bot config fetch helper
/grant alice @account:helper /bot
/grant alice @account:helper w:bot.config
```"#;
