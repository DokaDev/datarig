# Contributing to datarig

Thanks for your interest. datarig is early and changes quickly, so for anything larger than a
small fix, please open an issue first to agree on the approach.

## Build

You need the Rust toolchain of `rust-toolchain.toml` (rustup installs it on the first `cargo`
command), a C compiler and libclang (for the `pg_query` crate; see the README).

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

The PostgreSQL, PgBouncer and SSH integration tests need the services of `dev/` and the
environment variables listed in [dev/README.md](dev/README.md); without them they are skipped.
CI runs all of them, the performance budgets, `cargo deny check` and `cargo audit`.

- Unit tests live next to the code in `<module>/tests.rs`; integration tests in each crate's
  `tests/`.
- Screens are pinned with [insta](https://insta.rs) snapshots in English. After an intended
  rendering change run `INSTA_UPDATE=always cargo test -p datarig-tui` and review the snapshot
  diff as part of the change. Other languages are checked against their catalogs, not
  snapshots.
- `docs/keybindings.md` is generated from the keymap:
  `DATARIG_BLESS=1 cargo test -p datarig-tui --test keybindings_doc`.
- UI text goes through the catalogs in `locales/` (`en.toml` is the source of truth; every
  locale must have the same keys). See "UI strings (i18n)" in
  [docs/architecture.md](docs/architecture.md).

## Commits and pull requests

- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):
  `feat: …`, `fix: …`, `docs: …`, `test: …`, `refactor: …`, `perf: …`, `build: …`, `ci: …`,
  `chore: …`, with an optional scope (`fix(driver-postgres): …`).
- Keep a pull request to one change, with tests for what it fixes or adds.
- The code, comments and documentation are in English.

## Releases

Maintainers release by pushing a tag; `.github/workflows/release.yml` does the rest.

1. Set `version` in `[workspace.package]` of `Cargo.toml` (see "Versioning" in the README),
   run `cargo build` so that `Cargo.lock` follows, and merge that change.
2. Tag that commit `v<version>` and push the tag (`git tag v0.2.0 && git push origin v0.2.0`).

The workflow fails unless the tag is exactly `v` and the workspace version. It builds the
archives (macOS and Linux, arm64 and x86_64; Windows x86_64), installs and tests the Homebrew
formula from them, publishes the GitHub release with generated notes, then updates
`Formula/datarig.rb` in [DokaDev/homebrew-tap](https://github.com/DokaDev/homebrew-tap) over
SSH with that repository's deploy key (the `HOMEBREW_TAP_DEPLOY_KEY` secret). A
`vX.Y.0-rc.N` tag (with that version in `Cargo.toml`) is always a GitHub pre-release. Whether it
also updates the tap depends on whether a stable release exists yet: until the first one ships,
the tap tracks the latest release candidate so `brew install`/`upgrade` stays usable during this
pre-1.0 phase; once a stable release exists, later release candidates go back to being a GitHub
pre-release only and leave the tap alone. A pull request that changes the release files runs
everything except publishing.

## License

By contributing, you agree that your contributions are dual-licensed under the MIT and
Apache-2.0 licenses, as the rest of the project.
