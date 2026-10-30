# Documentation

This directory documents the current application: supported behavior, public
contracts, architecture, configuration, operations and reproducible development
procedures. Descriptions must agree with the implementation and its contract tests.
Update the relevant guide when behavior changes; link to the owning guide rather
than duplicating its details.

Review reports, finding ledgers, refactor plans, implementation diaries, task
status, one-off verification logs and comparisons between revisions do not belong
here. Use issues, pull requests and commit history for that material. Keep local
experiment output outside `docs/`. Compatibility and upgrade instructions belong
here when they describe formats and transitions the current code supports.

Start with [Development](development.md), then read [Architecture](architecture.md)
before extending the application.

- [Development](development.md): setup, daily workflow, coding conventions and adding features.
- [Architecture](architecture.md): module responsibilities, state ownership and concurrency.
- [Security](security.md): grants, groups, ownership, delegation and authentication.
- [Commands](commands.md): command syntax, permissions and behavior.
- [Generated command reference](command-reference.md): catalogue-derived syntax and permissions; regenerate with `auto/command-reference.py`.
- [Protocol](protocol.md): browser HTTP/WebSocket contracts and synchronization.
- [Bots](bots.md): bot API v1, scoped credentials, limits and retry guarantees.
- [Bot API JSON Schema](bot-v1.schema.json): machine-readable v1 request and response contracts.
- [Agent development](agent-development.md): reference worker and checkpoint responsibilities.
- [Media uploads](media-uploads.md): private S3 uploads, setup, limits and cleanup.
- [Data](data.md): persistence, supported upgrades, backups and moving the service.
- [PostgreSQL](postgresql.md): backend selection, offline SQLite transfer, connection settings and backups.
- [Server caches and system metrics](cache.md): cache reuse, limits, counters and operational commands.
- [Deployment](deployment.md): release builds, configuration, Nginx, Cloudflare Tunnel and object storage.
- [Testing](testing.md): required checks, isolated fixtures and regression contracts.
- [Storage benchmarks](storage-benchmarks.md): reproducible performance fixtures, work-count assertions and measurement limits.
