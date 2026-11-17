# PostgreSQL

**Versions.** CI tests datarig against PostgreSQL 17. Not supported before PostgreSQL 12: a
table's structure, its DDL, the key marks and copying rows as `INSERT` (datarig says the server
is too old).

A PostgreSQL profile has `driver = "postgres"`, and a URL starts with `postgres://`, for
example `postgres://me@db.internal:5432/app?sslmode=verify-full`
([Connections](connections.md)).

This page gathers what the rest of the guide says about PostgreSQL.

## Connecting

- Directly, or through an [SSH tunnel and bastion host](ssh-tunnels.md).
- Encrypted with [TLS](tls.md), as the profile's `sslmode` says.
- Behind PgBouncer or another [connection pooler](poolers.md), transaction pooling included.

## Safety

- Every statement is parsed with PostgreSQL's own parser before it is sent, and statements that
  may do harm ask first ([Environments and safety](environments.md)).
- A read-only profile refuses writes before anything is sent, and the server enforces it too.
- [Manual commit](manual-commit.md) keeps your changes in a transaction until you commit or
  roll it back; [Startup SQL](startup-sql.md) sets what each session starts with.

## Working with data

- The [SQL editor](editor.md) with vim keys and completion, the [results grid](results.md),
  [filtering and sorting a table](filtering.md) on the server, [editing data](editing.md)
  with the exact statements shown first, [export](export.md) and [charts](charts.md).
- [Query plans](plans.md): `EXPLAIN` and `EXPLAIN ANALYZE` as a tree and several other views.
- The [server monitor](monitoring.md) (PostgreSQL 12 or later): load, sessions, locks and
  replication.

Something missing or wrong? See the [FAQ](../faq.md), or open an issue on
[GitHub](https://github.com/DokaDev/datarig/issues).
