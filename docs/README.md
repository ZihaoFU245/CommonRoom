# Developer documentation

Start with [Development](development.md), then read [Architecture](architecture.md) before extending the application.

- [Development](development.md): setup, daily workflow, coding conventions, and adding features.
- [Architecture](architecture.md): module responsibilities, state ownership, and scaling limits.
- [Security](security.md): grants, groups, ownership, delegation, and persistence.
- [Bots](bots.md): scoped provisioning, machine credentials, frozen v1 contract, limits and retry guarantees.
- [Agent development](agent-development.md): minimal reference worker and checkpoint responsibilities.
- [Protocol](protocol.md): HTTP/WebSocket contracts and synchronization rules.
- [Testing](testing.md): required checks and regression coverage.
- [Backend review implementation](backend-review.md): finding-by-finding work and verification ledger.
- [Commands](commands.md): command syntax, permissions, and behavior.
- [Media uploads (draft)](media-uploads.md): private S3 uploads, setup, limits, cleanup and verification.
- [Data](data.md): persistence, upgrades, backups, and moving the service.
- [PostgreSQL review fixes](postgres-review.md): verified findings, corrections, tests and independent backend schemas.
- [PostgreSQL](postgresql.md): alternate backend, offline SQLite transfer, connection settings and backups.
- [Storage benchmarks](storage-benchmarks.md): mutation latency, write volume, memory, and remaining scaling costs.
- [Server caches and system metrics](cache.md): committed cache updates, reuse, lifetime counters and grant-authorized operational commands.
- [Deployment](deployment.md): release builds, configuration, Nginx, Cloudflare Tunnel, and object storage (AWS S3, Cloudflare R2, self-hosted RustFS).
