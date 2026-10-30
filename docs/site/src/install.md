# Install

## Homebrew

On macOS and Linux (arm64 and x86_64):

```sh
brew install dokadev/tap/datarig
```

Or add the tap once and use the short name from then on:

```sh
brew tap dokadev/tap
brew install datarig
brew upgrade datarig   # later, for a new release
```

The tap installs stable releases only. datarig is not in homebrew-core.

## Release archives

Download the archive for your platform from
[GitHub Releases](https://github.com/DokaDev/datarig/releases) and put the `datarig` binary on
your `PATH`. There are archives for macOS and Linux (arm64 and x86_64) and for Windows (x86_64,
untested). `SHA256SUMS` lists their checksums, for example:

```sh
shasum -a 256 -c SHA256SUMS --ignore-missing
```

Each archive also contains the licenses and the third-party notices.

## From source

You need:

- Rust **1.93** or newer. The repository's `rust-toolchain.toml` pins 1.93.0, and rustup
  installs it automatically.
- A C compiler and **libclang**, for the `pg_query` crate (it compiles PostgreSQL's parser and
  generates bindings with bindgen). On macOS the Xcode Command Line Tools have both; on
  Debian or Ubuntu, `apt install clang libclang-dev`. On Windows (untested outside CI), install
  LLVM and set `LIBCLANG_PATH` if bindgen does not find it.

```sh
git clone https://github.com/DokaDev/datarig
cd datarig
cargo install --locked --path crates/tui   # installs the datarig binary
# or: cargo build --release                # the binary is target/release/datarig
```

## Requirements

- A terminal of at least 80×24. A smaller window shows only a note saying how large it must
  be.
- A [Nerd Font](https://www.nerdfonts.com) is optional. datarig asks once whether your
  terminal shows its icons (see [FAQ](faq.md#icons-look-wrong)).

## Versions

Releases follow [Semantic Versioning](https://semver.org), as `0.MINOR.PATCH` until 1.0. A
patch release only fixes things. A minor release before 1.0 may change the configuration
format; datarig keeps reading older files and migrates them itself. For example, a version 1
config file is copied to `config.toml.v1.bak` before it is upgraded. A release candidate
(`vX.Y.0-rc.N`), when there is one, is a GitHub pre-release only. The Homebrew tap installs
stable releases. 1.0 is planned once several databases are supported and the configuration
is stable.

See [What's new](whats-new.md) for the changes in each release.
