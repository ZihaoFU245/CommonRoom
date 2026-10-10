# CommonRoom

A lightweight, command-driven chat application with a Rust server and a TypeScript/Preact web client. Nginx serves the web interface and proxies API/WebSocket requests to Rust; all server state lives in `data/`.

## Access Control

Access comes from **grants**: permission to do something in a particular place.
For example, `r:message.read` lets someone read messages in one room. Without a
matching grant, the server refuses the operation.

| Kind | Meaning | Example |
| --- | --- | --- |
| `r` | Read information | `r:message.read` |
| `w` | Create or change content | `w:message.create` |
| `x` | Perform a management operation | `x:member.add` |

Commands have their own grants too. To run `/history`, someone needs both the
`/history` command grant and `r:message.read` for the conversation. Checks apply
to the selected conversation and the command's target, so changing views does
not bypass access control.

### Groups and ownership

Groups are convenient bundles of grants:

- **user**: basic account and private-chat access. In a room, the user group
  allows reading, sending, replying, reacting, and retracting your own messages.
- **admin**: account and room administration. It does not automatically give
  access to other people's rooms or private messages, or allow retracting their
  messages. A room admin manages that room.
- **su**: all permissions everywhere. Only an existing su can add or remove
  another su. The server's stdin console belongs to su.

Creating a room makes you its owner, with participant and room-management grants.
New rooms are invitation-only: `/join` opens a room you already have access to.
The owner can invite people and transfer ownership.

### Common tasks

Run these examples in the server's stdin console as su. Accounts must already
exist; in the web UI, your grants must authorize the same actions.

Create a room and invite Bob with normal participant access:

```text
/new support
/add bob support
```

The room owner can let Alice invite others into that room:

```text
/add alice support
/grant alice support x:member.add
/grant alice support /add
```

Alice can then run `/add bob support`. Both grants are needed, along with room
participant access. Her global user or admin group does not give room access.

Give Alice the su group, or remove that assignment:

```text
/grant alice su
/revoke alice su
```

Make Bob read-only in `support` by replacing his room user bundle with individual
grants. This lets him find and open the room and read its history:

```text
/revoke bob user support
/grant bob support r:room.discover
/grant bob support x:room.join
/grant bob support r:message.read
/grant bob support /history
```

Grants **add together**. Revoking one grant does not remove access supplied by
another grant or group. Other room or global grants may still let Bob write.
`/permissions support` shows your effective access and, for authorized managers,
the room's grant assignments. `/kick bob support` removes Bob's room membership
and scoped grants; global grants still apply.

Use a room name to limit a grant to that room. `@global` applies globally;
`@account:alice` and `@private:alice:bob` identify one account or an existing
private conversation. Only su can change grants at those three scopes.

Granting a slash command does not grant its required actions. Command hints and
grant confirmations list those requirements; `/permissions` separates effective
actions, command grants, and assignments. `@server` remains a compatibility alias
for `@global`.

Use `/man grant`, `/man revoke`, or `/man permissions` for syntax and examples.
Use `/console` to open the Command view, and select a conversation in the sidebar
to return. `/rooms` lists rooms and their owners plus your own readable private
conversations; it excludes other people's private pairs, even for su.
See [the security guide](docs/security.md) for the full model.

See [docs/README.md](docs/README.md) for development, architecture, commands, testing, and deployment guides.
