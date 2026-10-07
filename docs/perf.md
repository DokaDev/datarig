# Performance measurements

`crates/datarig-bench` measures datarig the same way every time. Build it in
release mode; the scenarios that need PostgreSQL use `DATARIG_TEST_PG_URL` and the test
database of `dev/init` (the 4,000,000-row `analytics.events`).

```sh
cargo build --release -p datarig-tui -p datarig-bench
export DATARIG_TEST_PG_URL=postgres://datarig:datarig@127.0.0.1:55432/datarig
./target/release/datarig-bench all --out /tmp/perf.jsonl
```

| Scenario | What it does | What it reports |
|---|---|---|
| `rtt_ssh` | The same statements through an SSH tunnel: the proxy sits between the client and the SSH bastion of the tests, so it counts what crosses the SSH connection. Needs the bastion (`docker compose --profile ssh up -d --build ssh-bastion` in `dev/` after `dev/ssh/make-fixture.sh`), `DATARIG_SSH_FIXTURE`, `DATARIG_TEST_SSH_BASTION=127.0.0.1:52222` and the database as the bastion sees it, `DATARIG_BENCH_SSH_TARGET` (default `postgres:5432`) | Round trips of opening the tunnel and a session through it, and of each statement (the direct counts) |
| `rtt` | Runs statements on the driver's query session behind a TCP proxy that delays each direction by `--delay-ms` (30). The proxy counts *flights*: what the client sends before it hears from the server again. A result counts the flights until the proxy passed it on to the client, so a `COMMIT` sent right after it is not counted "to result" | Round trips until the result and in total (a trailing `COMMIT` or close included), milliseconds until the result, for a new and a prepared `SELECT`, the first page of a large result (not held, the default, and held: `hold_*`), the next page (fetched from a held portal, and run again past one not held: `resume_warm`), `INSERT`, and both inside a transaction block; and on the metadata session, the structure of a table the explorer opens (`table_structure`) the objects of a schema it opens, with their estimates (`schema_objects`), and the DDL of a table (`table_ddl`) |
| `rtt_mysql` | The MySQL driver's query session behind the same proxy (`DATARIG_TEST_MYSQL_URL`; any database the user may make a temporary table in, its rows are one of the session's own) | Round trips of opening a session, a small result, a page of a large one (not held: the session's `sql_select_limit` ends the statement with its first page), the next page run again past the rows it has (`resume`) and the statement after it, a held result and its next page, a count, `INSERT`, both inside a transaction, and a read-only profile's `SELECT` |
| `paging` | Runs `SELECT * FROM analytics.events` in a headless app with `paging = "hold"` and presses `G` in the grid until the result is complete (`--max-pages` stops earlier) | Time per page, the process's resident memory every 250 pages |
| `editor` | Opens a generated 5 MB SQL file in a headless app and draws each frame at 160x45 | Keystroke-to-frame time for typing, cursor movement, scrolling, Normal-mode edits and vim's other motions (`%`, `{`/`}`, `f`, `H`/`M`/`L`, `Ctrl+D`, a text object, `.`), for moving and scrolling among the hints of finished runs (every statement around the cursor has one), and of a theme switch; memory; autosave time |
| `editor_block` | The same file: Visual block operators over all of it (`gg Ctrl+V G $` with `y`, `d`, `u`, then `I` on every line), in a process of its own (the bench binary again), so the copies of the whole text it leaves in registers and undo steps never count in another scenario's memory | Key + frame time, the bytes one key's block operator walks; typing next to a hinted statement of 4.7 MB (after a 100,000-line `INSERT`, above the same `INSERT` on one line, above a statement under a 100,000-line comment header), with the bytes one key's hint checks lexed and the hint still there |
| `editor_mysql` | A generated 5 MB MySQL script (backtick names, `#` comments, backslash escapes, executable comments, variables, and stored procedures between `DELIMITER $$` lines) read as MySQL in a headless app at 160x45, in a process of its own (the bench binary again) so its memory never counts in another scenario's | Keystroke-to-frame time for the `editor` scenario's typing, cursor movement, scrolling, Normal-mode edits, vim's other motions and moving among run hints; memory; jumps to the end and back after an edit (each lexes the line states of the whole text again, the terminator a `DELIMITER` set carried in them) |
| `grid` | 2,000 rows × 24 columns of wide CJK text in the grid at 160x45 | Key + frame time |
| `plan` | A plan of 666 nodes (40 joins deep, each with a subplan, parallel workers, a CTE, an `Append` over 500 partitions) in each view of the Plan tab at 160x45 while the selection moves | Key + frame time, and the nodes each frame walked as a multiple of the plan's nodes |
| `chart` | 100,000 fetched rows (a timestamp and two numbers, spilled past the rows kept in memory) in the Chart tab as lines, bars and horizontal bars at 160x45 while the cursor moves, in a process of its own (the bench binary again), so the memory of its rows never counts in the `paging` scenario's | The frame that reads the rows into the chart, key + frame time after it, and the work each frame did (rows read, points and dot columns walked, cells painted) |
| `idle` | Runs the release binary in `tmux -L perf` with one connection and three tabs, still for `--idle-secs` (60), then 20 seconds with a paging countdown on screen (`paging = "hold"`) | Resident memory, CPU, event loop wakeups and frames per second |
| `startup` | Starts the release binary in `tmux -L perf` `--runs` times | Process start to first frame, binary size |

Every scenario prints the load average before it runs; `--out FILE` appends one JSON line per
scenario. Timings are reported as median and p95 over `--runs` repetitions.

The binary scenarios never touch the user's setup: a tmux server of their own (`-L perf`; `DATARIG_BENCH_TMUX=<name>` picks another, so two runs at once do not share sessions),
passwords in memory (`DATARIG_SECRET_STORE=memory`), and a config, data and state directory
under `--scratch` (a temporary directory by default). The binary writes its frame counters to
the file named by `DATARIG_BENCH_STATS` (off when unset).

## Budgets

`datarig-bench budget` runs the scenarios with the settings of
`crates/datarig-bench/budgets.toml` and fails when a measurement is over its budget.
CI runs it in the `perf` job of `.github/workflows/ci.yml` (Linux, PostgreSQL 17 with the
init scripts loaded, MySQL 8.4, tmux installed):

```sh
cargo build --release -p datarig-tui -p datarig-bench
DATARIG_TEST_PG_URL=postgres://datarig:datarig@127.0.0.1:55432/datarig \
DATARIG_TEST_MYSQL_URL=mysql://datarig:datarig@127.0.0.1:53306/datarig \
  ./target/release/datarig-bench budget
```

| Budget | Why it does not flake |
|---|---|
| Round trips of each kind of statement | Exact counts through the proxy, not times |
| Round trips through an SSH tunnel (`[rtt_ssh]`) | The same exact counts, with the proxy in front of the bastion: the tunnel adds latency, never a round trip |
| MySQL round trips (`[rtt_mysql]`) | Exact counts as for PostgreSQL: one per statement, the first page included; the next page asks the server about views and sets a larger limit before it runs again |
| Resident memory while paging 4,000,000 rows, and its growth from 125,000 rows on | Memory, not time; the growth must stay near zero because rows past `result_window_rows` live on disk |
| Keystroke-to-frame p95 in a 5 MB file | About 0.5 ms on a developer machine against a 25 ms budget |
| Visual block operators over the whole 5 MB file | The bytes one key walks are counted (in passes over the text, so a block operator that went line by line through the whole text would fail at once); the slowest key, about 130 ms on a developer machine, has a 500 ms budget |
| Idle memory, CPU and wakeups; frames per second with a paging countdown | Counts of the binary's own event loop (`DATARIG_BENCH_STATS`) |
| A chart of 100,000 rows (`[chart]`) | A frame draws from numbers read once per result and choice: its work is counted (cells and bars, at most what the screen holds), and reading the rows is counted per row; the key + frame time, under 0.5 ms on a developer machine, has a 25 ms budget |
| First frame p95 and binary size | The first frame has a wide margin (about 15 ms measured against 250 ms). The binary is held by the release profile (LTO, one codegen unit, symbols stripped): 9.5 MiB on macOS arm64 and 11.3 MiB on Linux arm64 at first (the CI's Linux x86_64 build reached 15.9 MiB by 0.6.0 and about 16.1 MiB with the plan views of 0.7.0, so the budget was 17 MiB; the MySQL driver of 0.12.0 adds about 0.5 MiB, so it is 19 MiB); without it, 20.7 MiB on the CI's Linux x86_64 |

To change a budget, measure first (`datarig-bench <scenario>` a few times on a quiet
machine), then keep the headroom the file's comments describe.
