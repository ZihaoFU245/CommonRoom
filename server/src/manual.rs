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
        "clean" => CLEAN,
        _ => {
            let name = format!("/{topic}");
            let command = commands::available(true, false)
                .iter()
                .find(|command| command.name == name)
                .ok_or_else(|| format!("No manual for '{topic}'. Use /man to list topics."))?;
            return Ok(format!(
                "# {}\n\n- {}\n\n## Usage\n\n```text\n{}\n```{}\n\n## Access required\n\n- The command grant must allow the selected conversation and its target.\n- The matching action grants must allow each affected resource.\n\n{}\n\n## See also\n\n- Use `/help` to list commands available here, or `/man permissions` to understand grants.",
                command.name,
                command.description,
                commands::usage_forms(command).join("\n"),
                match topic {
                    "user" => USER,
                    "room" => ROOM,
                    "retract" => RETRACT,
                    _ => "",
                },
                if command.requirements.is_empty() {
                    "- No additional action grant is required.".into()
                } else {
                    format!("- Requires: {}.", command.requirements)
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
        text.push_str(&format!("\n\n## {title}\n\n```text\n{actions}\n```"));
    }
    text.push_str("\n\n## Command grants");
    let catalog = commands::available(true, false);
    for section in ["General", "Account", "Rooms", "Messages", "Server"] {
        let names = catalog
            .iter()
            .filter(|command| command.section == section)
            .map(|command| format!("- `{}`", command.name))
            .collect::<Vec<_>>()
            .join("\n");
        text.push_str(&format!("\n\n### {section}\n\n{names}"));
    }
    Ok(text)
}

const OVERVIEW: &str = r#"# CommonRoom manual

- Use `/help` to list commands available in the selected conversation.

## Everyday commands

- `/man user` — list and manage accounts and scoped metadata.
- `/man room` — list and manage rooms and membership.
- `/man alias` — customize command prefixes in the web app.

## Usage

```text
/man [command|topic]
```

- Command names work with or without a slash: `/man grant` and `/man /grant` are equivalent.

## Access control topics

- `/man grant` — assign a group or add one permission.
- `/man revoke` — remove a group or a direct permission.
- `/man permissions` — understand r/w/x and browse the permission catalog.
- `/man groups` — compare user, admin, and su.
- `/man scopes` — choose where access applies.
- `/man ownership` — create rooms, invite people, and transfer ownership.
- `/man sudo` — run one command with temporary superuser authority.

## Other commands

- Use `/man history`, `/man user`, or another command name for its usage.
- `/help` lists commands available in the selected conversation.
- `/console` opens the Command view; select a room or use `/room join room` to return to a conversation.

- Manuals are public documentation.
- Reading one does not grant permission to run the command."#;

const USER: &str = r#"

## Examples

```text
/user ls @global
/user ls
/user ls #support
/user get @all
/user get bob @all
/user get bob media.max_size #support
/user set @all media.max_size 10m #support
/user set bob media.max_age 7d #support
/user set bob media.user_quota 1g
/user set bob label @unset
/user reset bob @all #support
/user reset bob password new-password
/user rename bob robert
/user new alice temporary-password admin
/user passwd old-password new-password
```

- Metadata is a two-level tree: a global root and sparse room override branches.
- Omitted scope uses the selected room; Console/stdin use `@global`.
- In private views, specify a room or `@global` for user metadata and lists.
- Use `#name` to select a room explicitly.
- `@all` selects every user in the scope, including the caller.
- `/user get bob #support` shows separate `@global` and `#support` sections.
- Global values have no hints; room values show `set here` or `inherited`.
- `get` without a key returns all visible keys in both sections.
- Room values override global values; unset values fall back without storing copies.
- Custom values inherit only where global account read authority allows inspection.
- Room deletion prunes its branch from every account atomically.
- Read your own metadata freely; other accounts require r:account.metadata (global/account) or room policy-management authority.
- Only su can change metadata, except `color` with `w:account.color.any` at the target scope.
- Room owners receive `w:account.color.any`: `/user set bob color blue #support` sets a member color, and `/user reset bob color #support` restores inheritance. Both support `@all` users.
- `/user reset bob media.max_size #support` removes one override and restores inheritance.
- `/user reset bob @all #support` clears only that room branch; `@global` clears only the root.
- `/user reset @all @all #support` resets every selected member's room branch.
- Empty branches are pruned; unrelated scopes and credentials remain unchanged.
- `/user reset bob password new-password` resets credentials using existing password-reset permissions.
- Only su may change a su account.
- Global admins receive `r:account.metadata` for inspection.
- Room policy managers may inspect members' room metadata.
- `x:account.metadata` is reserved for su; explicit grants do not bypass the su check.

## Media values

- `media.max_size`: size per file; accepts `k`, `m`, `g` units.
- `media.max_age`: duration, or `-1` for permanent.
- `media.user_quota`: root-only account-wide quota; accepts `k`, `m`, `g` units.
- `/user set bob media.user_quota 1g` targets account-wide metadata when scope is omitted.
- Explicit room scopes cannot set account-wide quota.
- Size and quota cannot exceed server ceilings.
- Room values override global values, then server defaults.
- `@unset` removes a value and restores inheritance.
- Custom lowercase keys accept single-token values up to 256 bytes.
- Use `@all` in the key position to read every key.
- `history.clean_min_age`: `0` disables cleanup, `-1` is unrestricted, or use a positive duration.
- Cleanup defaults follow current roles; derived defaults are never stored.
- Metadata never grants permissions.
- `get` shows effective media values, including inherited limits and server defaults.

- Bulk writes authorize all targets before committing; any rejection rolls back the entire operation.
- Renames preserve stable IDs, metadata, history and cursors.
- Passwords are masked in browser output.
- Deleting accounts needs confirmation in the browser and retains room messages attributed to the deleted account."#;

const ROOM: &str = r#"

## Examples

```text
/room ls
/room new #support
/room rename #support helpdesk
/room add bob #helpdesk
/room owner bob #helpdesk
/room join #helpdesk
/room leave #helpdesk
/room delete #helpdesk
```

## Scope and membership

- Explicit room scopes require `#name`; omitted scopes use the selected room.
- `/room add bob` invites Bob to the selected room.
- `/room add bob #support` invites Bob to a specific room.
- `add`, `kick`, `owner` and `leave` use the selected room when omitted.
- Listing shows authorized global rooms.
- Creation requires w:room.create at @global, assigns ownership and creates invitation-only admission.
- Rename/delete/invite/remove/owner operations require their matching room actions.
- Opening a room requires read/join access.
- Owners must transfer ownership before leaving.
- Deletion requires browser confirmation.

- Room identity, membership, grants, user metadata, history and read positions survive renames.
- Deletion removes room metadata; reusing a name inherits nothing."#;

const RETRACT: &str = r#"

## Behavior

- Retract a retained message by its ID.
- It disappears for every participant, and retained replies lose their quote preview.
- Read permission is required; own-message authority applies to your messages, while any-message authority also allows retracting other authors' messages.

- The web app asks for confirmation, including aliases and sudo.
- Stdin executes directly.
- Retraction cannot be undone.
- The message's hover action supplies its ID automatically in the web app."#;

const GRANT: &str = r#"# /grant

- Give someone a group of permissions, or one permission in a specific place.
- The account and resource must already exist.

## Bulk selection

- Use @all in the user field to select every active user in the requested scope.
- For example `/grant @all #support w:media.upload`, `/revoke @all user #support`, or `/grant @all admin` (global).
- All targets are checked against the original policy; any rejection rolls back the complete batch.
- Protected accounts, ownership, delegation and last-administrator guards still apply.

## Assign a group

```text
/grant username [user|admin|su] [#room|@global]
```

- Without a room, or with explicit `@global`, the group applies at global scope.
- With a room, user gives participant access; admin adds room management.
- Omitting the group means global admin. Write the group explicitly for clarity.

- Give Bob participant access to support:

```text
/grant bob user #support
```

- Give Alice every permission:

```text
/grant alice su
```

## Add one permission

```text
/grant username scope permission
```

- Use an exact action name or a slash command.
- For history access, grant both:

```text
/grant bob #support r:message.read
/grant bob #support /history
```

- These grants add access.
- They do not remove write access from another grant or group.
- Use `/man scopes` for room, global, account, and private-conversation scope names.

## Let someone invite users

- As the owner of support, give Alice participant access, then both invitation grants:

```text
/room add alice #support
/grant alice #support x:member.add
/grant alice #support /room
```

- Alice can now run `/room add bob #support`.
- Her global user or admin group does not replace room participant access.
- The `/room add` command grant alone does not grant `x:member.add`.
- Remove these direct grants with `/revoke alice #support x:member.add` and `/revoke alice #support /room`.

## Who can grant access?

- Only an existing su member, including stdin, can assign su. su is always global.
- Only su can change individual global, account, or private grants, or assign room admin.
- Global admin assignment requires `x:group.admin.assign`.
- Room managers need `x:policy.change` and can delegate only participant permissions they hold.
- The room owner may also grant or revoke `x:member.add` and `/room add` within that room, if they hold those permissions.
- Giving the room user group requires authority to delegate its full participant bundle.
- Command grants must allow the selected conversation and target; action grants must allow the target.

## Limit history cleanup

- Grants contain permissions only; set cleanup limits through user metadata.
- Use positive durations with s/m/h/d/w units.

- Upload permissions contain no quotas or retention values.
- Use `/user set bob media.max_size 25m #support` and `/user set bob media.max_age 7d #support`.
- `/revoke bob #support w:media.upload` blocks new uploads there; `/grant bob #support w:media.upload` restores permission.
- Metadata limits remain independent.

- This example permits cleanup of messages older than seven days; su must issue these grants:

```text
/grant bob #support x:history.clean
/user set bob history.clean_min_age 7d #support
/grant bob #support /clean
```

- An unrestricted action grant does not bypass metadata limits.

## Check access

- `/permissions #support` shows your effective access and, for authorized managers, the room's grants.
- See `/man permissions` for the catalog and `/man revoke` to remove access."#;

const REVOKE: &str = r#"# /revoke

- Remove a group assignment or a direct permission.
- Access from other grants and groups still applies.

## Bulk selection

- Use @all in the user field to select every active user in the requested scope.
- For example `/grant @all #support w:media.upload`, `/revoke @all user #support`, or `/grant @all admin` (global).
- All targets are checked against the original policy; any rejection rolls back the complete batch.
- Protected accounts, ownership, delegation and last-administrator guards still apply.

## Remove a group

```text
/revoke username [user|admin|su] [#room|@global]
```

- Remove Bob's participant bundle from support:

```text
/revoke bob user #support
```

- The exact assignment is removed.
- Omitting the group means global admin.
- Only su can remove su assignments.
- Ordinary admins cannot remove the last active admin.

## Remove one direct permission

```text
/revoke username scope permission
```

- Remove a direct send permission and a direct reply command grant:

```text
/revoke bob #support w:message.create
/revoke bob #support /reply
```

- This removes the named direct permission at that scope, including its cleanup-age variants.
- It does not remove the same permission supplied by a group or a global grant.
- The scope-management rules from `/man grant` apply here too.

## Make a participant read-only

- Replace Bob's room user bundle with grants to find, open, and read support:

```text
/revoke bob user #support
/grant bob #support r:room.discover
/grant bob #support x:room.join
/grant bob #support r:message.read
/grant bob #support /history
```

- Inspect `/permissions #support` for other direct, room admin, or global grants that may still allow writes.

## Remove room membership

- Revoking grants may leave the person's name in the member list.
- Use `/room kick` to remove membership and their direct/scoped group grants:

```text
/room kick bob #support
```

- Global grants still apply, except a room media-upload restriction overrides upload grants in that room (su remains fully trusted).
- Revocation takes effect on the next server check and updates connected clients.
- Content already received by a client cannot be taken back."#;

const PERMISSIONS: &str = r#"# Permissions

- A grant allows one action or command in a particular scope.
- Without a matching grant, access is denied.

## Three kinds of action

- `r` — read information, such as `r:message.read`.
- `w` — create or change content, such as `w:message.create`.
- `x` — perform a management operation, such as `x:member.add`.

## Commands need grants too

- Running `/history` requires both its command grant and `r:message.read` on the conversation.
- HTTP history requests check read access independently of command grants.

- Grants add together, without wildcards or groups nested inside groups.
- Room media has a narrow revocation override: `/revoke user #room w:media.upload` blocks uploads there until `/grant` restores them, even with a global upload grant.
- Su remains fully trusted.
- Other permissions keep their additive behavior.
- Global grants apply globally; resource grants apply only to the resource's stable identity.

## Message ownership

- `w:message.retract.own` allows retracting your own messages.
- `w:message.retract.any` allows retracting other authors' messages.
- Both also require read access and the `/retract` command grant.

- Authorship uses stable account IDs, so reusing a username does not inherit old messages.

## Inspect access

```text
/permissions #support
/permissions @groups
```

- `/permissions [scope]` shows your effective actions and command grants.
- `r:policy.read` allows viewing grant assignments at its own scope. Global policy-read grants do not disclose resource assignments; su can inspect all scopes. Read access alone shows only your effective access. Missing and inaccessible explicit policy scopes return the same error.
- `/permissions @groups` lists predefined bundles.
- `/permissions @audit` shows the audit log to su.

- See `/man grant`, `/man revoke`, `/man groups`, and `/man scopes`.
- The catalogs below list exact permission names."#;

const GROUPS: &str = r#"# Groups

- Groups are predefined bundles of grants.
- Their scope determines where the bundle applies.

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

## Private chats

- Private participants receive content grants in their original pair while they have a server group.
- Individual direct grants still add access.

- Additional explicit grants can extend these predefined bundles.
- Inspect actual grants before assuming fresh-account defaults.

## Manage groups

```text
/permissions @groups
/grant bob user #support
/revoke bob user #support
```

- See `/man grant` and `/man revoke` for authorization rules."#;

const SCOPES: &str = r#"# Scopes

- A scope is the place where a grant applies.

## Scope names

- `#support` — one room, identified internally by its stable ID.
- `@global` — global access across all resource scopes.
- `@account:alice` — one account, identified by its stable ID.
- `@private:alice:bob` — an existing private pair, using its original participant IDs.

- Deleting and recreating an account or room does not restore its old grants.
- Resource names and message IDs are identifiers, not credentials.

## Who can change each scope?

- Only su can change global, account, or private grants.
- Room managers can delegate their own participant permissions within their room.
- Room owners can also delegate `x:member.add` and `/room add` to let participants invite users.

## Selected conversation and target

- The selected conversation is where you invoke a command.
- An explicit argument can select a different target.
- Optional scopes use the selected conversation; Console/stdin use `@global`.
- User metadata/lists and bots require an explicit room or `@global` in private views.
- Grant/revoke keep explicit permission scopes; group assignments without a room stay global.
- Both places must allow the command, and each affected target must allow the action.
- The Command view does not bypass target checks.

- Cleanup across multiple conversations checks every target before changing any history.

## Examples

- Read one room:

```text
/grant bob #support r:message.read
/grant bob #support /history
```

- Reset Alice's password (grants issued by su):

```text
/grant bob @account:alice x:account.password.reset
/grant bob @account:alice /user
```

- Inspect an existing private pair (grants issued by su):

```text
/grant auditor @private:alice:bob r:message.read
/grant auditor @private:alice:bob /history
```

- Command invocation still needs its own grant in the selected context.
- Use `/man grant` for the full rules."#;

const SUDO: &str = r#"# /sudo

- Run one slash command with temporary superuser authority.

## Usage

```text
/sudo /command [arguments]
```

## Access required

- Both grants must apply at global scope.
- Only su can grant this authority:

```text
/grant bob @global /sudo
/grant bob @global x:command.sudo
```

- Neither grant alone permits elevation.
- User and admin groups do not include sudo; the su group includes both grants.

## Examples

```text
/sudo /room new support
/sudo /room join #support
/sudo /user reset alice password new-password
/sudo /permissions @audit
```

- The wrapped command receives all su command and action permissions.
- It can manage su accounts and grant permanent privileges, so sudo delegates full superuser trust.
- It bypasses command/action grants and cleanup metadata limits.
- The caller keeps their name and stable identity in messages, room ownership, and audit entries.
- Elevation does not itself change groups or persist any grants.
- Normal validation, storage rollback, and session checks still apply.
- `/sudo /room join room` saves the caller's membership and room user grants, so they retain participant access after elevation ends.
- Output is the wrapped command's result or error.

- For password commands, both sudo grants are rechecked after hashing.
- Nested sudo and ordinary message text are rejected.
- Browser-local clear and logout complete in the requesting tab after the server authorizes sudo.

## Revoke

```text
/revoke bob @global /sudo
/revoke bob @global x:command.sudo
```

- Revoking either grant blocks future sudo commands.
- Explicit group assignments and other additive grants still apply."#;

const CONSOLE: &str = r#"# /console

- Open the Command view in this browser tab.
- There is no Command button in the sidebar.

## Usage

```text
/console
```

- Accounts with no conversations start in this view automatically.
- Select a conversation in the sidebar, or use `/room join room`, to leave it.
- Each view keeps its own command output until the tab is refreshed.

## Access required

- The `/console` command grant must allow the selected context.
- The user group includes it by default.
- Opening the Command view does not add permissions.
- Commands still check their context, targets, and required actions.
- This command is available in the web UI only."#;

const OWNERSHIP: &str = r#"# Room ownership

- The person who creates a room owns it and receives participant and management grants.

## Create and invite

- Creating a room requires `w:room.create` at global scope and the `/room` command grant.
- Creation saves the room, stable identity, ownership, and grants together.

```text
/room new support
/room add bob #support
```

- New rooms are invitation-only.
- `/room join` opens a room you already have access to.
- Su accounts and `/sudo /room join room` callers can admit themselves, saving membership and room user grants.
- Ordinary joins do not add membership or grants.
- Another server admin does not automatically gain access to your new room.

## Owner permissions

- Invite or remove participants.
- Inspect and change participant grants.
- Delegate invitations with both `x:member.add` and `/room add`; see `/man grant` for the steps.
- Clean history and delete the room.
- Transfer ownership to another account.

- Ownership does not grant account/server management or permission to retract other authors' messages.

## Transfer ownership

```text
/room owner bob #support
```

- This transfers ownership and its management grants.
- Existing participant access remains.
- An owner must transfer ownership before leaving or being kicked.
- Ordinary room managers, including the owner, must transfer ownership before revoking the owner's room user group.
- Su authority, including /sudo, can revoke it without a transfer.

## Recovery

- Disabling or deleting an owner returns the room to the console. su has global recovery access.
- Use `/permissions #support` to inspect the room's grants."#;

const BOT: &str = r#"# /bot

- Manage bot accounts, tokens and external runtime settings.
- `[scope]` and `[key]` are optional.

## Accounts

- `/bot new name [scope]` — create a bot with read/send/reply/react and manual access.
- `/bot ls [scope]` — list bots; a room scope filters by membership.
- `/bot info name [scope]` — show owner, status, connections and permissions.
- `/bot owner name user` — assign the bot owner by stable account identity.
- `/bot rm name @global` — delete the account and its tokens.

- Scope defaults to the current room; Console/stdin use `@global`.
- In private views, specify a room or `@global`.
- Use `@global` explicitly to select global scope from a room.

## Tokens

- `/bot token new name label` — issue a token; save the secret shown once.
- `/bot token ls name` — list token IDs and labels.
- `/bot token rm name token-id` — revoke a token and disconnect its sockets.

- Tokens follow the account's current grants.

- Configure an online worker through its advertised chat commands, such as `@helper /config get` and `@helper /config set prompt "Help with Rust questions."`.
- The worker validates settings and enforces its owner/admin policy.

## Permissions

- Every operation needs `/bot` command access in its context and target.
- Accounts/tokens: global admin, su or stdin; ordinary account/scope permissions also apply. Tokens require `x:bot.credentials`.

- Use `/grant`, `/revoke`, `/room add`, `/room kick`, `/user disable`, `/user enable` and `/user rename` as for any user.

## Examples

```text
/bot new helper #support
/bot token new helper worker
@helper /config set prompt "Help with Rust questions."
@helper /config get
```"#;

const CLEAN: &str = r#"# /clean

- Delete history older than an age in an explicit scope.
- Both age and scope are required, even inside a room or private chat.

## Usage

```text
/clean age scope [--dry-run]
```

## Scopes

- `#support`: one room; the `#` prefix is required.
- `@direct:bob`: your existing private chat with Bob; requires a signed-in account.
- `@private:alice:bob`: one existing private pair, including authorized inspection.
- `@private`: all existing private conversations.
- `@global`: all rooms and private conversations.
- `@all` remains an explicit alias for `@global`.
- Missing private chats are rejected; cleanup never creates a conversation.

## Examples

```text
/clean -1 #support --dry-run
/clean 30d #support
/clean 7d @direct:bob --dry-run
/clean 7d @private:alice:bob
/clean 30d @private
/clean 30d @global --dry-run
```

## Permissions

- Require `/clean` command access in the invocation context and every target.
- Require `x:history.clean` on every target and an allowing `history.clean_min_age` value.
- `0` disables cleanup; `-1` allows any age; positive durations set a minimum message age.
- Defaults: admin/su `-1`, ordinary users `0`, room owners `-1` within their own room.
- Room metadata overrides global metadata, then live role defaults apply.
- Private conversations use the global cleanup limit.
- Su and authorized `/sudo` bypass command grants, action grants and metadata cleanup limits.
- `/clean -1 scope` explicitly wipes all history in the selected scope.
- Private participants do not receive cleanup authority automatically.
- Su can delegate private cleanup through explicit grants:

```text
/grant alice @private:alice:bob /clean
/grant alice @private:alice:bob x:history.clean
```

- Check all targets before counting or deleting; one denial rejects the entire batch.
- Storage failures roll back message deletion, attachment references, revisions and audit changes.

## Preview and results

- `--dry-run` checks the same permissions and counts matching messages and attachment references.
- Preview changes no messages, metadata, revisions or audit records.
- Execution reports deleted messages and removed attachment references as Markdown bullets.
- Shared attachments remain available while another retained message references them.
- Unreferenced files are cleaned asynchronously; quota is released after confirmed storage deletion.
- Cleanup does not change user metadata, grants or configured retention.
"#;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn every_manual_uses_bullets_for_explanations_and_fences_for_syntax() {
        let mut topics = vec!["overview", "groups", "scopes", "ownership"];
        topics.extend(
            crate::commands::available(true, false)
                .iter()
                .map(|c| c.name),
        );
        for topic in topics {
            let text = super::manual(Some(topic)).unwrap();
            let mut fence = false;
            for line in text.lines() {
                if line.starts_with("```") {
                    fence = !fence;
                    continue;
                }
                assert!(
                    fence || line.is_empty() || line.starts_with('#') || line.starts_with("- "),
                    "{topic}: unformatted explanation: {line}"
                );
            }
            assert!(!fence, "{topic}: unclosed code block");
        }
    }
}
