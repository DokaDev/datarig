# dev: test databases and services

Docker Compose services for developing and testing datarig: PostgreSQL 17 (with the test data
the integration tests expect), MySQL 8.4 (for the MySQL driver that is not written yet),
PgBouncer and an OpenSSH bastion. Every credential here is a throwaway for local testing.

## Start

```sh
cd dev
docker compose up -d        # the first start loads init/*.sql and init-mysql/*.sql (a minute or two)
docker compose ps           # wait until both databases are healthy
```

Optional services (compose profiles):

```sh
docker compose --profile pooler up -d pgbouncer

sh ssh/make-fixture.sh      # SSH keys for the bastion, written to ssh/.fixture (git-ignored)
docker compose --profile ssh up -d --build ssh-bastion
```

## Services

| Service | Host port | User / password | Database | Notes |
|---|---|---|---|---|
| PostgreSQL 17 (`datarig-pg`) | 55432 | `datarig` / `datarig` | `datarig` | scram-sha-256 authentication |
| MySQL 8.4 (`datarig-mysql`) | 53306 | `datarig` / `datarig` | `datarig` (tables in `shop`) | root password `datarig-root` |
| PgBouncer 1.25 (`datarig-pgbouncer`, profile `pooler`) | 56432 | `datarig` / `datarig` | `datarig` | transaction mode, no prepared statements, round robin over three server connections, `options` dropped |
| OpenSSH bastion (`datarig-ssh-bastion`, profile `ssh`) | 127.0.0.1:52222 | `tunnel` (key) / `pw` (password `datarig-pw`) | | forwards only to `postgres:5432` and `pgbouncer:5432` |

- PostgreSQL: `postgres://datarig:datarig@127.0.0.1:55432/datarig`
- PgBouncer: `postgres://datarig:datarig@127.0.0.1:56432/datarig`
- MySQL: `mysql://datarig:datarig@127.0.0.1:53306/datarig`

## Data

- PostgreSQL (`init/*.sql`): the `shop` schema (users, products, orders, order_items, reviews,
  audit_log without a primary key, the `order_summary` view), `analytics.daily_stats`, and
  `analytics.events` with 4,000,000 rows for paging, cancel and memory tests. The first eight
  users are hand-made edge cases: CJK text, emoji and ZWJ sequences, combining marks, full- and
  half-width characters, tabs and newlines in values, NULLs and very long text.
- MySQL (`init-mysql/*.sql`): a comparable `shop` schema and about 1,000,000 `events` rows.

The integration tests create and drop their own tables; they leave `public` empty.

## Environment variables for the tests

| Variable | Value |
|---|---|
| `DATARIG_TEST_PG_URL` | `postgres://datarig:datarig@127.0.0.1:55432/datarig` |
| `DATARIG_TEST_POOLER_URL` | `postgres://datarig:datarig@127.0.0.1:56432/datarig` |
| `DATARIG_TEST_SSH_BASTION` | `127.0.0.1:52222` |
| `DATARIG_SSH_FIXTURE` | the directory `ssh/make-fixture.sh` printed |
| `DATARIG_REQUIRE_PG`, `DATARIG_REQUIRE_POOLER`, `DATARIG_REQUIRE_SSH` | `1` makes a missing service a failure instead of a skip |

## Stop and reset

```sh
docker compose --profile pooler --profile ssh down      # stop; the data stays in named volumes
docker compose --profile pooler --profile ssh down -v   # also delete the volumes: the next start loads the init scripts again
```

The data lives in the named volumes `datarig-pgdata` and `datarig-mysqldata`. CI does not use
this file: `.github/workflows/ci.yml` starts the same images as service containers.
