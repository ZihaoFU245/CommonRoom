# Commonroom 0.3.0

- Invite codes support self-service registration. Authorized room senders can
  notify readers with `@all`.
- Browser snapshots support ETag revalidation. WebSockets can resume from a
  recent checkpoint and send smaller message deltas.
- Browser media and avatar link/info routes have moved to the batch resolve
  endpoints. Bot v1 retains its individual link/info routes.
- The library supports `release_and_remove` for releasing ownership and removing
  an asset from the library in one operation.
- `GET /api/health` now returns plain text `ok`. Update probes that parse JSON.
- WebSocket clients can opt into `conversation_delta` frames and resume with
  the snapshot's `resume` token. Room messages can carry `mention_all`.

SQLite requires schema v21. Existing v21 databases open directly. To upgrade a
v20 database, back it up and open it with `release-0.2.1` before starting 0.3.0.
See [Data](docs/data.md) for compatibility and migration details.
