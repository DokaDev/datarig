# Contributing to datarig

Thanks for your interest. datarig is early and changes quickly.

## Before you open a pull request

datarig is maintained by one person for now, so changes need to be planned together before
any code is written. As more maintainers join, this process will be made lighter. Please:

1. **Open an issue first** (or comment on an existing one) describing what you want to change
   and why.
2. **Wait for the discussion and an assignment.** Some areas are being reworked, and the issue
   is where you'll find out whether your change overlaps with work in progress.
3. **Then open the pull request** and name its issue in the description with `Part of #123`.
   Please don't use closing keywords such as `Closes` or `Fixes`: the maintainers decide when
   an issue is done.

Pull requests that arrive without an agreed issue may wait a long time or be closed, even when
the code is good: with a single maintainer, unplanned changes are hard to review and to fit
into the plan. Small fixes such as typos or broken links can skip the issue: say so in the
description with the line `no issue: <reason>`.

Planned and in-progress work is tracked in
[the issues](https://github.com/DokaDev/datarig/issues?q=is%3Aissue+is%3Aopen+label%3Aroadmap),
so it's worth a look before you start. Thank you for bearing with this while the project is
small.

## Finding your way around

To get oriented in the code, [DeepWiki](https://deepwiki.com/DokaDev/datarig) offers an
auto-generated technical overview of it; the user guide is the
[docs site](https://dokadev.github.io/datarig/), and the code is the source of truth.

## Build

You need the Rust toolchain of `rust-toolchain.toml` (rustup installs it on the first `cargo`
command), a C compiler, clang and libclang (for the `pg_query` crate; see
[Install](https://dokadev.github.io/datarig/install.html#from-source) on the docs site).

```sh
cargo build
cargo run -p datarig-tui -- --help
```

## Test

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The PostgreSQL, PgBouncer, MySQL, SSH and TLS integration tests need the services of `dev/` and
the environment variables listed in [dev/README.md](dev/README.md); without them they print
`SKIPPED`. CI runs all of them, the performance budgets, `cargo deny check` and `cargo audit`.
A pull request that changes only `README.md`, `CONTRIBUTING.md`, `docs/` or changelog
fragments skips the builds: CI then runs the docs site, the tests that check the pages against
the code (`book_refs`, `book_screens` and the generated pages), `rustfmt`, `cargo deny`,
`actionlint` and the docs rule; the skipped jobs conclude `skipped`, which counts as passing.

The integration job of `.github/workflows/ci.yml`, run locally against the services of `dev/`
(the values are those of [dev/README.md](dev/README.md)):

```sh
cd dev
docker compose up -d                         # PostgreSQL and MySQL
docker compose --profile pooler up -d pgbouncer
sh ssh/make-fixture.sh                       # prints the fixture directory
docker compose --profile ssh up -d --build ssh-bastion
sh tls/make-certs.sh                         # prints the certificates' directory
sh tls/up.sh tls/.fixture                    # the scratch TLS servers
cd ..

export DATARIG_TEST_PG_URL=postgres://datarig:datarig@127.0.0.1:55432/datarig
export DATARIG_TEST_POOLER_URL=postgres://datarig:datarig@127.0.0.1:56432/datarig
export DATARIG_TEST_MYSQL_URL=mysql://datarig:datarig@127.0.0.1:53306/shop
export DATARIG_TEST_MYSQL_ADMIN_URL=mysql://root:datarig-root@127.0.0.1:53306/
export DATARIG_TEST_MYSQL_CLIENT='docker exec -i datarig-mysql mysql -udatarig -pdatarig shop'
export DATARIG_TEST_SSH_BASTION=127.0.0.1:52222
export DATARIG_SSH_FIXTURE=$PWD/dev/ssh/.fixture
export DATARIG_TEST_PG_TLS_URL=postgres://datarig:datarig@localhost:55433/datarig
export DATARIG_TEST_MYSQL_TLS_URL=mysql://datarig:datarig@localhost:53307/datarig
export DATARIG_TEST_MYSQL_TLS_ADMIN_URL=mysql://root:datarig-root@localhost:53307/
export DATARIG_TEST_TLS_CERTS=$PWD/dev/tls/.fixture
# a missing service fails its tests instead of skipping them
export DATARIG_REQUIRE_PG=1 DATARIG_REQUIRE_POOLER=1 DATARIG_REQUIRE_MYSQL=1 DATARIG_REQUIRE_SSH=1 DATARIG_REQUIRE_TLS=1
cargo test --workspace --locked

# each driver's suite again with every connection going through a dialer, as through a tunnel
DATARIG_TEST_DIAL=tcp cargo test -p datarig-driver-postgres --test integration_pg --locked
DATARIG_TEST_DIAL=tcp cargo test -p datarig-driver-mysql --test integration_mysql --test definer_mysql --locked
# the MySQL suite once more over TLS
DATARIG_TEST_MYSQL_SSLMODE=PREFERRED cargo test -p datarig-driver-mysql --test integration_mysql --test definer_mysql --locked
```

The performance budgets of `crates/bench/budgets.toml`, as the `perf` job runs them (tmux,
PostgreSQL with `dev/init/*.sql` loaded, MySQL and the SSH bastion above; see
[docs/perf.md](docs/perf.md)). Note the MySQL database: `datarig`, not `shop`.

```sh
cargo build --release --locked -p datarig-tui -p datarig-bench
DATARIG_TEST_PG_URL=postgres://datarig:datarig@127.0.0.1:55432/datarig \
DATARIG_TEST_MYSQL_URL=mysql://datarig:datarig@127.0.0.1:53306/datarig \
DATARIG_TEST_SSH_BASTION=127.0.0.1:52222 DATARIG_SSH_FIXTURE=$PWD/dev/ssh/.fixture \
DATARIG_SECRET_STORE=memory \
  ./target/release/datarig-bench budget
```

A time over its budget (ms, rows per second) is measured once more and fails only if it is over
budget again; counts, memory and the binary size fail at once (docs/perf.md, "Budgets").

- Unit tests live next to the code in `<module>/tests.rs`; integration tests in each crate's
  `tests/`.
- Screens are pinned with [insta](https://insta.rs) snapshots in English. After an intended
  rendering change run `INSTA_UPDATE=always cargo test -p datarig-tui` and review the snapshot
  diff as part of the change. Other languages are checked against their catalogs, not
  snapshots.
- Some docs are generated, and a test fails when one is out of date. Regenerate with
  `DATARIG_BLESS=1 cargo test -p datarig-tui --test <test>`:

  | Generated file | From | `<test>` |
  |---|---|---|
  | `docs/keybindings.md` | the keymap, actions and `:` commands | `keybindings_doc` |
  | `docs/themes.md` | the theme tokens | `themes_doc` |
  | `docs/site/src/reference/configuration.md` | `crates/core/src/config/doc.rs` and the `:set` settings | `config_doc` |
  | `docs/site/src/reference/command-line.md` | `datarig --help` and `ENV_VARS` (`crates/tui/src/lib.rs`) | `cli_doc` |
  | `docs/site/src/screens/*.txt` | the snapshots listed in `tests/book_screens.rs` | `book_screens` |

  The docs site (`docs/site`, [mdBook](https://rust-lang.github.io/mdBook/)) is built with
  `mdbook build docs/site`; `tests/book_refs.rs` checks that its hand-written pages, the README
  and this file name only commands, actions, keys and config keys that exist.
- UI text goes through the catalogs in `crates/i18n/locales/`, one file per area in each language
  (`en/` is the source of truth; every locale must have the same keys, each in the file of the
  same name). See "UI strings (i18n)" in
  [docs/architecture.md](docs/architecture.md).

## Commits and pull requests

- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):
  `feat: …`, `fix: …`, `docs: …`, `test: …`, `refactor: …`, `perf: …`, `build: …`, `ci: …`,
  `chore: …`, with an optional scope (`fix(driver-postgres): …`).
- Open the issue first (see "Before you open a pull request") and name it in the pull request
  description with `Part of #123`, not with a closing keyword such as `Closes` or `Fixes`. The
  `PR rule` check (`.github/workflows/pr-rule.yml`) fails until the description names an issue
  (`#123`, `owner/repo#123` or the issue's link) or has the line `no issue: <reason>`; a
  release pull request has `no issue: release`. Like the docs rule, the line counts anywhere
  in the description, in any case, but not inside an HTML comment or a code block, and editing
  the description runs the check again.
- Keep a pull request to one change, with tests for what it fixes or adds.
- The code, comments and documentation are in English.

## Documentation and changelog

The user documentation is the site in `docs/site` (built with
[mdBook](https://rust-lang.github.io/mdBook/)), published at
<https://dokadev.github.io/datarig/>. Preview it with the mdBook release that CI pins (the
version in `.github/scripts/docs-tools.sh`, from the
[mdBook releases](https://github.com/rust-lang/mdBook/releases)):

```sh
mdbook serve docs/site
```

- Generated pages are never edited by hand; each has a test that fails when it is out of date
  (the table under "Test" says what each comes from). Regenerate them all with
  `DATARIG_BLESS=1 cargo test -p datarig-tui --test <test>` for each of `keybindings_doc`,
  `themes_doc`, `config_doc`, `cli_doc` and `book_screens`, and commit the result.
- `tests/book_refs.rs` checks the hand-written pages, the README and this file: every inline
  code span that looks like a name of the app (a `:` command, an action id, a context, a config
  key or table, a key chord, a theme name) must exist, and key chords must be bound by
  default. A span meant as something else goes in its allow-list, with the reason. The same test
  rejects Hangul and absolute local paths in the docs, `changes/` and `.github/`, and checks
  that every include of the site resolves.
- A screen on a page is the text of a snapshot test: add the screen's name and the snapshot to
  `SCREENS` in `tests/book_screens.rs`, generate it with the `book_screens` command above, and
  show it in a text block that includes `../screens/<name>.txt`. After a snapshot changes, the
  screens are regenerated the same way.
- CI builds the site on every push and pull request and checks every link and anchor offline.
  The site is deployed only from a stable release tag, by the release workflow.

### The README recordings

The GIFs and stills of the README are never captured by hand: CI records them with
[VHS](https://github.com/charmbracelet/vhs) from the tapes in `docs/demo/tapes`, against the
made-up data of `docs/demo/compose.yml` (its own containers and ports, apart from `dev/`), and
the release workflow publishes them with the docs site under `demo/`. A tape that watches the
server (`monitor`) records under a load of its own (`docs/demo/load/monitor.sh`, run by the
`demo-load` service of the same compose file): `render.sh` starts it just before that tape and
removes it right after, so the other tapes record an idle server; it reaches only the demo
databases, and changes none of their data. Every tape waits for the text it expects on the
screen, so a change that moves a screen fails the `Demo` workflow (`.github/workflows/demo.yml`)
until the tape is updated. It runs on pull requests that change `docs/demo` and on the release
pull request; run it by hand from the Actions tab to preview the recordings of a branch (they
are in the run's `demo` artefact). On a pull request, the `Demo preview` workflow then puts the
run's stills in one comment (updated in place on each run). The render also makes `social.png`,
the repository's social preview (uploaded by hand in the repository's settings).

The site is published only from a stable release tag, so a new tape's recording is online from
the next stable release on; until then a link to it in the README shows nothing. The `docs site`
job of CI checks that every `https://dokadev.github.io/datarig/demo/` file the README and the
site show is made by a tape (`.github/scripts/demo-refs.sh`): `<name>.gif` by
`docs/demo/tapes/<name>.tape`, `<name>.png` by a tape's `Screenshot <name>.png`. To record
locally, with
`vhs`, `ttyd`, `ffmpeg`, JetBrainsMono Nerd Font and Python 3 with Pillow (for `social.png`)
installed:

```sh
sh docs/demo/make-certs.sh     # the demo databases' TLS certificates, in docs/demo/.certs
docker compose -f docs/demo/compose.yml up -d --wait
DATARIG_BIN=target/release/datarig docs/demo/render.sh demo-out        # or: ... demo-out hero
docker compose -f docs/demo/compose.yml down
```

### The docs rule of pull requests

`.github/workflows/docs-rule.yml` checks every pull request (and again when its description
is edited):

- A change users can see (the English catalog, key bindings, actions, `:` commands, config and
  policy keys, connection profiles, the command line and environment variables, themes) also
  changes a hand-written page under `docs/site/src` (generated pages, screens and What's new do
  not count), or the description has the line `no doc change: <reason>`.
- A change to `crates/` adds a changelog fragment, or the description has the line
  `no changelog: <reason>`.
- A release pull request leaves no fragments in `changes/`, and What's new has the version's
  section.

The two lines count anywhere in the description, in any case, but not inside an HTML comment
or a code block. A pull request stacked on another is judged against its base branch as it is
now; after that branch is rewritten, rebase onto it.

### Changelog fragments

A fragment is a file `changes/<kind>/<slug>.md`, `<kind>` one of `added`, `changed`,
`fixed`, `removed` and `security`, with one or more bullets in English written for users (see
[changes/README.md](changes/README.md)). A change of the configuration formats always has a
`changed` fragment that says so, and that datarig migrates the old files, so that What's
new and the release notes tell users. `scripts/changes.sh check changes/*/*.md` checks them
locally. A fragment merged after the release pull request goes into the next release.

### Public text style

Text that users read (the README, this file, the docs site, changelog fragments, the catalogs
in `crates/i18n/locales/`, the screens, `--help`, the Cargo descriptions) uses British spelling and no em
dash. In place of an em dash write a colon, a comma, parentheses or a new sentence; a title
that names its subject takes a colon ("Apply changes: shop.users"). An en dash in a range
(1–5) and `--` in a command line are fine. British spelling: colour, behaviour, centre,
favourite, cancelled, labelled, organise, analyse, materialised. Names stay exactly as they
are, in code font where a page shows them: config keys, theme tokens and tables (`color`,
`[colors]`), commands and their arguments (`:explain analyze`), SQL (`ANALYZE`), actions, file
names. A settings label may say "Colour" for the `color` key.

`python3 scripts/text-style.py` checks it (the `docs site` job of CI runs it): no em dash in
those files or in the string literals of `crates/` (comments are not checked), and no US
spelling in the prose of the pages, the fragments and the English catalog, leaving out code
blocks, `inline code`, `{placeholders}`, URLs and words in capitals. A name that must keep a
US spelling outside code goes in its `ALLOW_PHRASES` or `ALLOW_KEYS`, with the reason.
`--counts` lists the findings per file.

## Releases

Maintainers release by pushing a tag; `.github/workflows/release.yml` does the rest.

1. In one pull request: set `version` in `[workspace.package]` of `Cargo.toml` (see
   [Versions](https://dokadev.github.io/datarig/install.html#versions) on the docs site), run
   `cargo build` so that `Cargo.lock` follows, and run `scripts/changes.sh release <version>`,
   which moves the changelog fragments into a new section of What's new
   (`docs/site/src/whats-new.md`) and deletes them. Merge it.
2. Tag that commit `v<version>` and push the tag (`git tag v0.2.0 && git push origin v0.2.0`).

The release notes are that section of What's new, the paragraph about the archives, then the
list of pull requests GitHub generates. A tag whose version has no section fails before the
builds. An urgent patch without fragments still gets its one line:
`scripts/changes.sh release 0.36.1 --note "Fix …"`. A release candidate (if one is ever
tagged) needs no section: its notes are GitHub's list only, and the fragments wait for the
stable release.

The workflow fails unless the tag is exactly `v` and the workspace version. It builds the
archives (macOS and Linux, arm64 and x86_64; Windows x86_64), installs and tests the Homebrew
formula from them, publishes the GitHub release, then updates
`Formula/datarig.rb` in [DokaDev/homebrew-tap](https://github.com/DokaDev/homebrew-tap) over
SSH with that repository's deploy key (the `HOMEBREW_TAP_DEPLOY_KEY` secret). A
`vX.Y.0-rc.N` tag (with that version in `Cargo.toml`) is always a GitHub pre-release. Whether it
also updates the tap depends on whether a stable release exists yet: until the first one ships,
the tap tracks the latest release candidate so `brew install`/`upgrade` stays usable during this
pre-1.0 phase; once a stable release exists, later release candidates go back to being a GitHub
pre-release only and leave the tap alone. After a stable release (not a release candidate),
the docs site is built from the tag and deployed to GitHub Pages; a failed deploy leaves the
release and the tap alone and can be re-run. A pull request that changes the release files runs
everything except publishing and deploying.

## License

By contributing, you agree that your contributions are dual-licensed under the MIT and
Apache-2.0 licences, as the rest of the project.
