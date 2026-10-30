# Plans

datarig shows a statement's query plan as a tree and as several other views. It works on
PostgreSQL and on MySQL 8.0.16 and later; not on MariaDB.

## Getting a plan

| Keys | Command | PostgreSQL | MySQL |
|---|---|---|---|
| `Space e x` | `:explain` | `EXPLAIN (FORMAT JSON)` | `EXPLAIN FORMAT=TREE` |
| `Space e a` | `:explain analyze` | `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` | `EXPLAIN ANALYZE FORMAT=TREE` (8.0.18 and later) |

Both work on the statement under the cursor, through the same checks as any run. An
`EXPLAIN (FORMAT JSON …)` on PostgreSQL, or an `EXPLAIN ANALYZE` or `EXPLAIN FORMAT=TREE` on
MySQL, that you type yourself shows as a plan the same way.

### `ANALYZE` runs the statement

- **PostgreSQL**: a write under `ANALYZE` is rolled back, and a read-only profile refuses it.
- **MySQL**: a write runs in a transaction of its own that is rolled back (inside your open
  transaction, under a savepoint rolled back to). A read-only profile refuses it for a write or
  a call of a function that is not built in. What a rollback cannot undo is not run:
  `EXPLAIN ANALYZE` of a write that names a table of another engine than InnoDB (MyISAM,
  MEMORY, …; NDB is fine), a view, or a table with triggers is refused, saying which; so is one
  whose table's triggers your account cannot see (without the `TRIGGER` privilege every
  `EXPLAIN ANALYZE` of a write is refused, and `EXPLAIN` without `ANALYZE` still shows the
  plan). A call of a function that is not built in asks first.

### A text `EXPLAIN` you ran

A text `EXPLAIN` stays rows, with a line under them. `P` in the results (or `Space e p`)
plans that same statement again as a plan: with `FORMAT JSON` on PostgreSQL, keeping its other
options; with `EXPLAIN FORMAT=TREE` (never with `ANALYZE`) on MySQL. It never happens by itself.

- It goes without a question only when the wrapped statement is a plain `SELECT` the app would
  also run again for a next page (the same allowlist, its server check included: no view, no
  user operator or overloaded name). Like that allowlist it trusts functions to be labelled
  honestly, so a function wrongly declared `IMMUTABLE` or `STABLE` can still run while the
  statement is planned.
- Anything else asks first, saying what runs again (`ANALYZE` runs the statement, rolled back;
  an `EXECUTE`'s parameters or what a view hides are evaluated to plan it, not rolled back).
- It is refused when the tab ran other statements since, when the result came with other
  statements, or when the tab's connection, database or session changed.
- On MySQL, `INTO @v`, `FOR CONNECTION`, an `EXPLAIN` of a table and an executable comment are
  refused.

## Reading the plan

The Plan tab shows the plan as a tree with each node's estimated and actual rows (and loops),
its self and total time, a bar of its share of the whole colored by heat, and its buffers.

- Nodes that take a fifth of the time or more are marked hot.
- An estimate ten times off or more is marked with how far and which way. Fewer rows than
  estimated under a `LIMIT`, or on the inner side of a semi or anti join, is no misestimate:
  the node was stopped early.
- Without `ANALYZE` the numbers are the planner's estimated costs, and the tab says so.
- `Enter` shows the selected node's detail: conditions, filters and the rows they removed,
  buffers, workers, loops, output. `h` and `l` close and open its children.

```text
{{#include ../screens/plan-tree-postgres.txt}}
```

### Views

`v` and `V` cycle the views; the digits pick one.

| Key | View |
|---|---|
| `1` | The tree |
| `2` | Summary cards (the slowest node and its share, the worst row estimate, disk reads and the cache hit ratio, planning and execution time) above a compact tree |
| `3` | An icicle (each node as wide as its share of its parent's time) |
| `4` | A flame graph |
| `5` | A timeline: each node's time to its first and its last row, per loop (needs `ANALYZE`; else the estimated startup and total cost, said so) |
| `6` | The row flow: the rows each node passes up as a band, thick by their number, with the rows its filters removed |
| `7` | A treemap (area = each node's own time) |
| `8` | A box diagram (boxes joined by lines; `<` and `>` and the wheel move around a large plan) |
| `9` | Raw: the plan as `psql` prints it (written from the JSON, nothing run again), or MySQL's own text |

`y` copies the plan as the raw view shows it, `Y` as the server sent it (its JSON, or its
text). Large plans stay readable and fast: names are cut, layouts are bounded, and only what is
on screen is drawn.

### On MySQL

The plan is the server's tree. The raw view and both copies are the server's text as it came;
there are no buffers and no planning or execution time (the measured total is shown), and a
node the server gives no cost (a sort, a subquery's head) shows its children's together marked
`≈`, never as a number of the server's.

```text
{{#include ../screens/plan-tree-mysql.txt}}
```

## Not yet

- Plans on MariaDB (its plans are shaped otherwise): the plan actions are not offered there,
  and `EXPLAIN` runs and shows its text.
- Comparing two plans of a statement (before and after a change).
