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
| `rtt` | Runs statements on the driver's query session behind a TCP proxy that delays each direction by `--delay-ms` (30). The proxy counts *flights*: what the client sends before it hears from the server again. A result counts the flights until the proxy passed it on to the client, so a `COMMIT` sent right after it is not counted "to result" | Round trips until the result and in total (a trailing `COMMIT` or close included), milliseconds until the result, for a new and a prepared `SELECT`, the first page of a large result, the next page, `INSERT`, and both inside a transaction block |
| `paging` | Runs `SELECT * FROM analytics.events` in a headless app and presses `G` in the grid until the result is complete (`--max-pages` stops earlier) | Time per page, the process's resident memory every 250 pages |
| `editor` | Opens a generated 5 MB SQL file in a headless app and draws each frame at 160x45 | Keystroke-to-frame time for typing, cursor movement, scrolling and Normal-mode edits; memory; autosave time |
| `grid` | 2,000 rows × 24 columns of wide CJK text in the grid at 160x45 | Key + frame time |
| `idle` | Runs the release binary in `tmux -L perf` with one connection and three tabs, still for `--idle-secs` (60), then 20 seconds with a paging countdown on screen | Resident memory, CPU, event loop wakeups and frames per second |
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
init scripts loaded, tmux installed):

```sh
cargo build --release -p datarig-tui -p datarig-bench
DATARIG_TEST_PG_URL=postgres://datarig:datarig@127.0.0.1:55432/datarig \
  ./target/release/datarig-bench budget
```

| Budget | Why it does not flake |
|---|---|
| Round trips of each kind of statement | Exact counts through the proxy, not times |
| Round trips through an SSH tunnel (`[rtt_ssh]`) | The same exact counts, with the proxy in front of the bastion: the tunnel adds latency, never a round trip |
| Resident memory while paging 4,000,000 rows, and its growth from 125,000 rows on | Memory, not time; the growth must stay near zero because rows past `result_window_rows` live on disk |
| Keystroke-to-frame p95 in a 5 MB file | About 0.5 ms on a developer machine against a 25 ms budget |
| Idle memory, CPU and wakeups; frames per second with a paging countdown | Counts of the binary's own event loop (`DATARIG_BENCH_STATS`) |
| First frame p95 and binary size | Wide margins (15 ms and 5.5 MiB measured against 250 ms and 16 MiB) |

To change a budget, measure first (`datarig-bench <scenario>` a few times on a quiet
machine), then keep the headroom the file's comments describe.
