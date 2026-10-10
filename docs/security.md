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
| `user` at server scope | Account directory, own password, private sends, general commands |
| `admin` at server scope | User grants, room/account creation, enable/disable/delete accounts, admin group assignment, config inspection |
| `user` at room scope | Discover/read/join the room, member list, send/reply/react, retract own messages, corresponding commands |
| `admin` at room scope | Room user grants plus room management and grant inspection |
| `su` at server scope | Every action and command at every scope, including other users' private history and retraction |

Private participants receive the user content bundle within their own pair while
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
/sudo /new support
```

Only su can grant global permissions. User and admin groups do not include sudo;
su includes both grants. A room-scoped grant does not authorize elevation.
The wrapped command needs no separate command or action grants. It retains the
caller's name and stable identity in messages, ownership and audit entries.
Elevation itself does not change persisted group assignments or permissions,
and is cleared on success, rejection and storage failure. The command can still
explicitly assign permanent privileges, so sudo delegates full superuser trust.

Password commands release temporary authority before hashing and recheck the
session, both sudo grants and target identity before committing. Revoking either
grant rejects a queued operation. Nested sudo and ordinary message text are
rejected. Read `/man sudo` for examples and revocation syntax.

## Resources and ownership

Creation requires both `w:room.create` at server scope and permission to invoke
`/new` in the current context. Creating a room atomically creates its ID, creator
ownership, participant assignment and room-management grants. New rooms are
invitation-only. There is no public self-admission mode.

The owner can invite/remove participants, inspect and change participant grants,
clean history, delete the room, or transfer ownership. Ownership does not grant
retraction of other authors' messages or account/server management. A transfer
moves only the creator's management grants; existing participant access remains.
Owners must transfer ownership before leaving or being kicked. Disabling or deleting an account
returns owned rooms to the console and removes the deleted account's grants.

Starting a new private pair requires `w:private.create`; sending to an existing
pair requires that pair's `w:message.create` and the invoked command grant.

`/join` opens an already authorized room; it does not manufacture membership or
grants. Room names, message IDs and knowledge of a private-pair key are not access
credentials. Explicit read grants also allow a room to appear in a user's list.
Member lists require their own read permission.

## Actions and commands

The permission catalog has individual actions in three families:

- `r`: `room.discover`, `message.read`, `message.metadata`, `member.list`,
  `account.list`, `server.config`, `policy.read`.
- `w`: `message.create`, `message.react`, `message.retract.own`,
  `message.retract.any`, `room.create`, `private.create`, `account.password.own`.
- `x`: `member.add`, `member.remove`, `room.join`, `room.delete`,
  `room.owner.transfer`, `history.clean`, `account.create`,
  `account.password.reset`, `account.disable`, `account.enable`, `account.delete`,
  `group.admin.assign`, `group.su.assign`, `policy.change`, `command.sudo`.

Prefix the action with its family, for example `r:message.read`. A slash command
name such as `/history` is a separate permission. No matching grant means denial.
Use `/man grant`, `/man revoke`, `/man permissions`, `/man groups`, `/man scopes`
and `/man ownership` for the built-in manuals. `/grant user group [room]` assigns
a group; `/grant user scope permission [minimum-age]` adds an individual grant.
`/revoke` has the corresponding forms.

Grants are additive: there are no deny rules or recursive groups. `/revoke`
removes direct grants; it does not subtract permissions supplied by a group.

The evaluator checks the command in its invocation context and on each resolved
target, then checks the target action. Selecting one room does not authorize an explicit target
in another room. The Command view can invoke commands available in an authorized
scope, but command and action checks still apply on the actual target. Multi-conversation cleanup authorizes
all targets before mutating any. HTTP history and read tracking require resource
read access independently of slash-command grants.

`/console` opens the browser's Command view after checking its command grant in
the selected context. It gives no additional access. `/rooms` lists only the
signed-in account's readable private pairs, even when su or explicit grants allow
inspection of other pairs. Inspection grants still apply to those resources.

Scopes accepted by policy commands are a room name, `@global` (legacy alias `@server`),
`@account:username`, or an existing `@private:a:b` pair. Server grants apply across
target scopes; room/account/private grants apply only to the identified resource.
Only `su` can edit server, account, or private grants. Room managers can delegate
only participant actions and commands they themselves hold. `/add` and room user
group assignment require authority to delegate the complete participant bundle.
Owners may additionally grant or revoke `x:member.add` and `/add` in their own
room, provided they hold those permissions. This lets room participants invite
others without granting policy management or other room-management powers.
Ownership is checked by the account and room's stable IDs. Assigning a scoped
`admin` management bundle requires `su`.

## Commands and examples

```text
/grant bob su
/revoke bob su
/grant bob admin support
/revoke bob admin support
/grant bob @global w:room.create
/grant bob @global /new
/owner bob support
/permissions support
/permissions @groups
/permissions @audit
```

The first two commands require `su`. Omitting the group from `/grant bob` or
`/revoke bob` means `admin` at server scope, for compatibility. Room group
assignments add membership; revoking an assignment removes its grants, while the
membership label remains until `/kick` or `/leave`.

Make an existing participant read-only by removing the room user assignment and
adding the exact read and command grants:

```text
/revoke bob user support
/grant bob support r:message.read
/grant bob support r:room.discover
/grant bob support x:room.join
/grant bob support /history
```

`/history` is allowed in that room; sending, replying, reacting and retracting are
not. Add `r:member.list` and `/members` separately if membership should be visible.
Remove direct read access with `/revoke bob support r:message.read`.

A superuser can delegate cleanup without other management powers:

```text
/grant bob support x:history.clean 7d
/grant bob support /clean
```

The minimum age is a typed constraint on cleanup only. Bob can delete messages
older than seven days in `support`, but cannot use a shorter age or clean another
room. `/permissions support` shows effective permissions, applicable assignments,
and scoped direct grants with constraints when the requester can inspect policy.
`/permissions @groups` lists the predefined bundles. `@audit` is restricted to `su`.

## Persistence and revocation

Schema v5 renames stored `/permit` and `/unpermit` command permissions to `/grant`
and `/revoke`, preserving scopes, constraints, ownership, identities and read cursors.
The old commands are no longer accepted. Audit records retain their original names.

Schema v4 migrates old memberships to room user assignments and old admin flags to
server admin assignments. Existing rooms become console-owned with explicit
legacy admin management/content grants, preserving the old ability to join them.
Old admins' password-reset and global cleanup powers become explicit account
grants that `su` can remove. Newly created rooms do not inherit legacy grants.
A v3 migration preserves read cursors; only v1/v2 migrations create read baselines.

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
