# Explorer

The explorer on the left lists, in sections that open and close (and stay closed across
restarts): **Connections**, **Tunnels** ([SSH tunnels](ssh-tunnels.md)), **Environments**
([Environments and safety](environments.md)), **Startup SQL** ([Startup SQL](startup-sql.md))
and **Saved queries**. Each section's head shows its name and how many it has.

Under a connected profile it lists the server's databases, their schemas, tables and views
(materialized views with an icon of their own). `j`/`k` move, `l`/`h` open and close, `Enter`
opens or toggles, `r` reloads. The full key list is in the
[keybindings reference](../reference/keybindings.md).

```text
{{#include ../screens/explorer-structure.txt}}
```

## Marks

- A profile through an SSH tunnel carries the tunnel mark (`SSH` with icons off), and one with
  read-only rules a lock in blue (`RO` with icons off). That comes from the rules alone, never
  from an environment's name.
- `◆` after a profile marks an open transaction.

## Row and size estimates

Each table and materialized view shows the server's estimates of its rows and its size on disk,
dim and on the right (`orders   ~50k rows · 19 MB`). They are read with the schema's list in the
same round trip, from the statistics that `VACUUM` and `ANALYZE` keep, without opening the table
and without a lock on it. A table that has grown to more than twice the rows its last `ANALYZE`
saw shows the live rows of the server's statistics instead. A table with no statistics yet shows
nothing, and the status bar says "rows and size unknown (no statistics yet)".

The largest tables of each schema stand out. With `[explorer] size_heat = "3"` (the default),
the largest tenth of those with a known size (one at least) shows its estimates in amber from
1 MiB and in red from 1 GiB. `"5"` (or `:set explorer.size_heat=5`) uses five levels: yellow
from 1 MiB, amber from 100 MiB, orange from 1 GiB, red from 10 GiB. `"off"` keeps them all dim.

## A table's structure

An open table shows its structure, read from the catalog in one round trip the first time it
opens (`r` reads it again; the table itself is never read). The groups:

- **Columns**: key marks, type, `not null`, default.
- **Primary Key** and **Foreign Keys**: `Enter` on a foreign key goes to the referenced table.
- **Indexes**: columns or expressions with their order, operator class and collation (as in
  `(created DESC)`), `UNIQUE`, the method, a partial predicate, and whether a key or constraint
  owns it.
- **Unique** and **Check Constraints**.
- **Triggers**: timing, events (with the columns of `UPDATE OF`), row or statement, the
  function, disabled; a trigger with a `WHEN` condition opens to it.

Each key, index and check opens to the columns it covers, in its order (a foreign key's columns
with the column each references, as `user_id → users.id`). Views show their columns and
triggers, materialized views their columns and indexes.

The groups start closed, one line each (`Columns (27)`); what you open stays open for the
session. Because the explorer is narrow, the status bar shows the whole line under the cursor.

**A lookup never waits on another session.** A table another session locks (an `ALTER TABLE`,
a `VACUUM FULL`, a migration waiting for its lock) says the structure is unavailable because
the table is locked, without queueing behind that lock: at once when the lock is already there,
after at most 2 s when it is taken while the structure is read.

## Filter

`/` filters the explorer by a part of a name, ignoring case: profiles (by name, host or
database), folders (a matching folder shows everything in it), tunnel presets, environments,
Startup SQL presets and saved queries, and under a connected profile the databases, schemas,
tables, views and structure items the explorer has already read. Nothing is asked of the
server: a schema whose tables were never listed, a table whose structure was never opened, and
the tree of a profile that is not connected are not searched. Matches show with the way to them
opened; `Esc` gives the tree back as it was.

## Show DDL

`D` on a table, view, materialized view, index or trigger (or its menu, or `:ddl [schema.name]`)
opens its `CREATE` statements in a read-only tab; `F` on a trigger shows its function's.

- On PostgreSQL the DDL is rebuilt from the catalog in one round trip and marked as such
  in a first comment line: owned sequences, columns with
  their collations, defaults, identity and generated expressions, constraints, partitioning or
  inheritance, storage parameters, owner, indexes, triggers, row-level security and policies,
  comments and grants. Not included: rows, sequence values, rules, extended statistics, security
  labels. Every name is qualified.
- On MySQL it is the server's own `SHOW CREATE` text (see [MySQL](mysql.md#show-ddl)).
- The text can be moved in, searched and yanked; edits are refused. `o` opens it in a new
  console on the same connection, and `r` reads it again.
- Reading never waits on another session: when the object, or a table its definition reads, is
  locked, the tab says so and shows nothing.

Not yet: DDL of objects the explorer does not list (functions on their own, sequences, types).

## Layout

`Space b` hides or shows the explorer, and `Space <` / `Space >` (or a drag of its right border)
change its width; both are kept across restarts.
