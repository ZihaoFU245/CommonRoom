# Trust and permissions

Authorization belongs to `server/src/engine/authorization.rs`. The persisted policy
contains scoped grants and group assignments; the engine compiles these into
per-account, per-scope action and command masks. Checks do not scan all grants or
query SQLite. The compiled index is outside serialized message state, and changes
only after a successful policy write. Ordinary message sends do not recompile it.

## Principals and groups

Accounts and rooms have stable IDs. Private conversations use the stable IDs of
their original two participants. Message ownership uses an author ID, independently
of its display name. Deleting and recreating an account or room does not restore
old grants. The console has the distinct principal ID `console`, assigned to `su`.
Its permissions use the same grant evaluator as web accounts.

The three predefined groups are fixed grant bundles, not authorization bypasses:

| Group and assignment | Grants |
| --- | --- |
| `user` at server scope | Account directory, own password and rename, private sends, general commands |
| `admin` at server scope | User grants, room/account creation, account enable/disable/delete, admin group assignment, config inspection |
| `user` at room scope | Discover/read/join the room, member list, send/reply/react, upload media, retract own messages, corresponding commands |
| `admin` at room scope | Room user grants plus room management and grant inspection |
| `su` at server scope | Every action and command at every scope, including other users' private history and retraction |

Private participants receive the user content bundle, including uploads, within their own pair while
they have a server group assignment. Other accounts need explicit read grants;
`su` has these globally. Superusers see other private pairs as `@private:a:b` views
in their conversation list. They can inspect, react, reply, send, clean, and retract
there without changing the pair's original participants.

Only an existing `su` member (including stdin) can assign or revoke `su` membership.
`su` cannot be assigned at room scope. Ordinary admins cannot reset, enable,
disable, delete, or change group assignments for a `su` account. Disabled superusers
retain this protection even though they cannot exercise any permissions. These
safeguards stop account-management commands from impersonating a superuser. The
console remains a recovery path if all web superusers are removed or disabled.

A newly provisioned admin does not automatically get password-reset privileges or
access to someone else's rooms. Password reset requires a separate grant.
Ordinary admins cannot revoke, disable or delete the last active administrator;
`su` can, since stdin provides recovery.

## Temporary superuser commands

`/sudo /command [arguments]` runs one command with su command and action
permissions. Both `/sudo` and `x:command.sudo` must be granted at global scope:

```text
/grant bob @global /sudo
/grant bob @global x:command.sudo
/sudo /room new support
```

Only su can grant global permissions. User and admin groups do not include sudo;
su includes both grants. A room-scoped grant does not authorize elevation.
The wrapped command needs no separate command or action grants. It retains the
caller's name and stable identity in messages, ownership and audit entries.
Elevation itself does not change persisted group assignments or permissions,
and is cleared on success, rejection and storage failure. A failed policy rebuild
replaces compiled authority with an empty deny-all index. The command can still
explicitly assign permanent privileges, so sudo delegates full superuser trust.

Password commands release temporary authority before hashing and recheck the
session, both sudo grants and target identity before committing. Revoking either
grant rejects a queued operation. Nested sudo and ordinary message text are
rejected. Read `/man sudo` for examples and revocation syntax.

## Resources and ownership

Creation requires both `w:room.create` at server scope and permission to invoke
`/room` in the current context. Creating a room atomically creates its ID, creator
ownership, participant assignment and room-management grants. New rooms are
invitation-only. There is no public self-admission mode.

The owner can invite/remove participants, inspect and change participant grants,
clean history, rename/delete the room, or transfer ownership. Ownership does not grant
retraction of other authors' messages or account/server management. A transfer
moves only the creator's management grants; existing participant access remains.
Owners must transfer ownership before leaving or being kicked. Disabling or deleting an account
returns owned rooms to the console and removes the deleted account's grants.
Ordinary room managers, including the owner, must transfer ownership before
revoking the owner's room `user` assignment. Su authority, including `/sudo`,
can revoke that assignment without transferring ownership.

Starting a new private pair requires `w:private.create`; sending to an existing
pair requires that pair's `w:message.create` and the invoked command grant.

`/room join` opens an already authorized room for ordinary accounts without adding
membership or grants. Su accounts and `/sudo /room join room` callers can admit themselves;
this saves their membership and room user assignment, preserving access after sudo
ends. Room names, message IDs and knowledge of a private-pair key are not access
credentials. Explicit read grants also allow a room to appear in a user's list.
Member lists require their own read permission.

## Actions and commands

The permission catalog has individual actions in three families:

- `r`: `room.discover`, `message.read`, `message.metadata`, `member.list`,
  `account.list`, `server.config`, `policy.read`.
- `w`: `media.upload`, `message.create`, `message.react`, `message.edit.own`, `message.edit.any`, `message.retract.own`,
  `message.retract.any`, `room.create`, `private.create`, `account.password.own`, `account.rename.own`,
  `account.color.own`, `account.color.any` (room-only).
- `x`: `member.add`, `member.remove`, `room.join`, `room.delete`,
  `room.rename`,
  `room.owner.transfer`, `history.clean`, `account.create`,
  `account.password.reset`, `account.disable`, `account.enable`, `account.delete`,
  `group.admin.assign`, `policy.change`, `command.sudo`,
  `account.rename.any`, `bot.credentials`.

Prefix the action with its family, for example `r:message.read`. A slash command
name such as `/history` is a separate permission. No matching grant means denial.
Use `/man grant`, `/man revoke`, `/man permissions`, `/man groups`, `/man scopes`
and `/man ownership` for the built-in manuals. `/grant user group [room]` assigns
a group; `/grant user scope permission` adds an individual grant.
`/revoke` has the corresponding forms.

Grants are additive, without recursive groups. Normally `/revoke` removes direct
grants without subtracting group permissions. Room uploads have one narrow
exception: `/revoke user #room w:media.upload` records a room restriction overriding
all upload grants there, except su authority. `/grant` restores that user's room
uploads. Other actions retain their additive semantics.

The evaluator checks the command in its invocation context and on each resolved
target, then checks the target action. Selecting one room does not authorize an explicit target
in another room. The Command view can invoke commands available in an authorized
scope, but command and action checks still apply on the actual target. Multi-conversation cleanup authorizes
all targets before mutating any. HTTP history and read tracking require resource
read access independently of slash-command grants.

`/console` opens the browser's Command view after checking its command grant in
the selected context. It gives no additional access. `/room ls` lists only the
signed-in account's readable private pairs, even when su or explicit grants allow
inspection of other pairs. Inspection grants still apply to those resources.

Scopes accepted by policy commands are a room name, `@global` (legacy alias `@server`),
`@account:username`, or an existing `@private:a:b` pair. Server grants apply across
target scopes; room/account/private grants apply only to the identified resource.
Only `su` can edit server, account, or private grants. Policy inspection
requires `r:policy.read` at the specific resource scope or su authority; global
policy-read grants only permit inspection of global assignments. Read access
alone permits viewing the caller's effective access without disclosing assignments.
Missing and inaccessible explicit policy scopes return the same error.
Room managers can delegate only participant actions and commands they themselves
hold. `/room add` and room user
group assignment require authority to delegate the complete participant bundle.
Owners may additionally grant or revoke `x:member.add` and `/room add` in their own
room, provided they hold those permissions. This lets room participants invite
others without granting policy management or other room-management powers.
Ownership is checked by the account and room's stable IDs. Assigning a scoped
`admin` management bundle requires `su`.

## Commands and examples

```text
/grant bob su
/revoke bob su
/grant bob admin #support
/revoke bob admin #support
/grant bob @global w:room.create
/grant bob @global /room
/room owner bob #support
/permissions #support
/permissions @groups
/permissions @audit
```

The first two commands require `su`. Omitting the group from `/grant bob` or
`/revoke bob` means `admin` at server scope, for compatibility. Room group
assignments add membership; revoking an assignment removes its grants, while the
membership label remains until `/room kick` or `/room leave`.

Make an existing participant read-only by removing the room user assignment and
adding the exact read and command grants:

```text
/revoke bob user #support
/grant bob #support r:message.read
/grant bob #support r:room.discover
/grant bob #support x:room.join
/grant bob #support /history
```

`/history` is allowed in that room; sending, replying, reacting and retracting are
not. Add `r:member.list` and `/user ls` separately if membership should be visible.
Remove direct read access with `/revoke bob #support r:message.read`.

A superuser can delegate cleanup without other management powers:

```text
/grant bob #support x:history.clean
/grant bob #support /clean
```

Su sets the independent limit with `/user set bob history.clean_min_age 7d #support`.
Bob can delete messages at least seven days old, provided command and action
grants also permit it. Metadata `0` disables cleanup; `-1` allows any age, including
`/clean -1 #support` to wipe that scope. `/permissions #support` shows grants;
`/user get bob #support` shows limits and their source. An unrestricted action
grant cannot override a metadata restriction. Su and authorized `/sudo` bypass
both grant and cleanup-limit checks.
`/permissions @groups` lists the predefined bundles. `@audit` is restricted to `su`.

## Persistence and revocation

Login uses the same 1–32 Unicode character name rules as account creation.
Missing and disabled accounts perform dummy Argon2 verification with the default
password work parameters before returning the common invalid-credentials error.

The current HEAD schema is v19. New empty databases initialize transactionally; existing databases must already be v19. Legacy migration paths are removed. Unsupported schemas are rejected without modification, as described in [Data](data.md).

Policy updates, ownership changes and their audit records are saved atomically.
Failed writes preserve both persisted and compiled permissions. The audit retains
the latest 1000 successful privileged operations/policy changes, without message
bodies or passwords. It is an operational record, not a tamper-proof external log.

Snapshots include groups, policy revision, per-conversation permissions and command
lists. Connected clients lose inaccessible history and controls after revocation.
The server rechecks session, command, action and target identity after asynchronous
password work. Revocation cannot erase content already delivered to a client.

The server operator and `su` remain fully trusted. Messages and sessions remain in
the server database; this change is access control, not end-to-end encryption.

Account rename preserves stable account and private conversation IDs and all scoped grants. Sessions, memberships, message display authors, reply authors, recipients, reactions, mention metadata, private keys and read cursors move to the new name atomically. Historical message text and audit records retain their original contents. Own rename is included in user/admin groups; any-account rename requires explicit delegation or su.

Room rename requires the separate `x:room.rename` action on the room's stable
scope and `/user rename` in the invocation context and on the target. It is included
in the room management bundle and newly created/transferred owner grants.
It gives no account rename authority. Renaming preserves all scoped grants and
room identity; reusing the old name creates a distinct resource with no inherited
access or read cursors. Existing explicit owner grants are not rewritten: su may
delegate `x:room.rename` and `/user rename` when absent.

## Bot connections

Bots are ordinary accounts, without a bot/human flag. /bot lifecycle and credential management additionally require global admin or su membership (or stdin); room admin and a direct /bot grant alone are insufficient for those operations. Worker configuration uses mention commands and the worker's verified owner/admin policy. Legacy bot.config grants remain parseable for saved-policy compatibility but no longer expose remote configuration. The global admin bundle includes x:bot.credentials for account-wide credential management. Existing protected-su account checks apply even while targets are disabled.

New room bots receive explicit direct grants rather than the user bundle: discover/read/member-list/join/send/react and /help, /man, /whoami, /user ls, /history, /room join, /reply, /react. Settings, message retraction/cleanup, private creation and administrative commands are excluded. Explicit @global creation applies these grants globally and requires the creator to hold each corresponding permission/command there. Ordinary admins with only room access cannot delegate global content access.

Credential authentication resolves stable account ID to the current active username, then uses the same command execution, history/read checks, and snapshot filtering as browser sessions. Credentials cannot specify an author or bypass grants. Only hashes of 244-bit random secrets (two UUIDv4 values) are stored. Revocation is checked on subsequent actions and WebSocket updates/heartbeats; revocation broadcasts wake connected sockets. Already-delivered content cannot be recalled. Account-wide input throttling is shared across credentials and sockets. See [Bots](bots.md).

Direct bot access: a bot running on the configured `trust` socket IP may use HTTP/WS directly against `/api/bot/v1/` (including the configured base path) without `X-Forwarded-Proto`. Bearer authentication and current account grants remain required. Socket identity, not X-Forwarded-For, authorizes this exception. Browser endpoints still require forwarded HTTPS; a bot request carrying X-Forwarded-Proto must also indicate https. With the default bind/trust of 127.0.0.1, same-machine agents can connect directly; when trust names a remote proxy, a local process must use the proxy or originate from the configured trusted IP.

`w:message.edit.any` permits changing another author's retained message when
`r:message.read` and `/edit` are also granted on that conversation. It does not
change authorship or grant access to other conversations. Like other powers
outside the participant bundle, only su/console may delegate it; a room manager
cannot grant it even when holding it. Ordinary participants retain edit-own.

Session-bearing GET endpoints require a configured Origin or, when browsers omit
Origin, `Sec-Fetch-Site: same-origin`. Same-site sibling requests are rejected.
Bearer-authenticated reads retain their credential checks without requiring Origin.

## Name colors

Active accounts own their root `color` value and can set/reset it without an action
grant. Root color ownership grants no authority in a room. Room `color` metadata
overrides the root, then falls back to normal text color when both are absent.

`w:account.color.own` allows setting or clearing your own override in one room.
`w:account.color.any` allows setting or clearing members' room overrides, including
bulk color updates; owners receive it as a transferable ownership grant. Su retains
room authority. Neither action is valid at global/account/private scopes, and neither
appears in global or room group bundles. First admission creates a direct room
own-color grant. Owners holding `color.any` and policy-management authority can grant
or revoke that capability even if they no longer hold `color.own` themselves.

`/revoke bob #support w:account.color.own` removes the direct grant; no global or
group color rights remain to restore it implicitly. Existing room overrides stay
in place, so changing the root cannot replace a managed room color. Re-adding an
existing member or assigning a user/admin group does not restore own-color access.
Regrant explicitly with `/grant bob #support w:account.color.own`. Normal additive
semantics remain; another explicit `color.any` grant still authorizes room writes.
Color grants never authorize other metadata keys, another room, or someone else's
root metadata. Non-su writers cannot manage protected su accounts. Other metadata
writes remain su-only.

## Media grants

Room user/admin groups include `w:media.upload` by default, separately from text
send rights. A room owner can use `/revoke user #room w:media.upload` to block that
user's uploads and new media references while preserving text/read access. It
overrides explicit/global upload grants in that room, except su authority, and
persists across restarts, renames and group reassignments. `/grant` restores access.
Private participants also have uploads by default. `/grant user scope
w:media.upload [max-bytes] [max-age]` allows typed media limits (age units s/m/h/d/w,
or -1). Omitted limits use server defaults. A request must fit one additive grant's
size/age pair and the server size ceiling. Explicit room limits replace the default
room allowance. Room delegation cannot exceed the
actor's own media allowance. Allocation, completion and newly added message
references recheck current media/read/send rights. Downloads require current read
rights and an unexpired retained reference, independently of upload grants.
See [Media uploads](media-uploads.md).


User metadata writes are reserved for su (including stdin and authorized sudo).
Global admins receive `r:account.metadata` only. Room policy management permits
room-member metadata inspection, not changes; a direct `x:account.metadata`
flag cannot bypass the su boundary. Metadata never changes command/action grants.
Cleanup always names its target. Private cleanup requires both `/clean` and
`x:history.clean` on that pair. Bulk operations authorize every target before
counting/deleting. Preview uses the same checks and writes no state or audit.

Sessions are indexed by domain-separated Blake2s token digests. Root-path production
deployments use `__Host-chat_session` with Secure, HttpOnly and Path=/; scoped
deployments retain `chat_session`. Duplicate selected cookie names are rejected
across all Cookie headers. The bounded durable authentication audit records verified
login outcomes and valid logouts without credentials or request bodies. See [Data](data.md).
