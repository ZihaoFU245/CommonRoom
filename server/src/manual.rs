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
        _ => {
            let name = format!("/{topic}");
            let command = commands::available(true, false)
                .into_iter()
                .find(|command| command.name == name)
                .ok_or_else(|| format!("No manual for '{topic}'. Use /man to list topics."))?;
            return Ok(format!(
                "# {}\n\n{}\n\n## Usage\n\n```\n{}\n```\n\n## Access required\n\n- The command grant must allow the selected conversation and its target.\n- The matching action grants must allow each affected resource.\n\n{}\n\n## See also\n\nUse `/help` to list commands available here, or `/man permissions` to understand grants.",
                command.name,
                command.description,
                command.usage,
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

Start with `/man grant` to give access, or `/man revoke` to remove it.

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

## Other commands

Use `/man history`, `/man reset`, or another command name for its usage.
`/help` lists commands available in the selected conversation.
`/console` opens the Command view; select a room or use `/join room` to return to a conversation.

Manuals are public documentation. Reading one does not grant permission to run the command."#;

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

- At global scope: account directory, own password, new private chats, and general commands including `/man`.
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
/new support
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
