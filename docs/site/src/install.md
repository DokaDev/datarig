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

Each archive also contains the licences and the third-party notices.

## From source

On Linux, the Homebrew tap and the release archives above need no build tools. To build
datarig yourself you need:

- Rust **1.93** or newer, from [rustup](https://rustup.rs) (the Rust of a Linux distribution is
  often older). The repository's `rust-toolchain.toml` pins 1.93.0, and rustup installs it
  automatically.
- A C compiler, **clang** and **libclang**, for the `pg_query` crate: it compiles PostgreSQL's
  parser and generates its bindings with bindgen, which loads libclang and needs clang's own
  headers. On macOS the Xcode Command Line Tools have all three. On Linux, with git:

  ```sh
  sudo apt install build-essential clang libclang-dev git     # Debian, Ubuntu
  sudo dnf install gcc clang clang-devel git                  # Fedora
  sudo pacman -Syu --needed base-devel clang git              # Arch Linux
  ```

  On Arch Linux the command upgrades the whole system with the packages: installing them
  against fresh package lists without upgrading the rest is a partial upgrade, which Arch does
  not support.

  On Windows (untested outside CI), install LLVM and set `LIBCLANG_PATH` if bindgen does not
  find it.

```sh
git clone https://github.com/DokaDev/datarig
cd datarig
cargo install --locked --path crates/tui   # installs the datarig binary
# or: cargo build --release                # the binary is target/release/datarig
```

CI checks these commands by building datarig with them in clean Debian, Ubuntu, Fedora and
Arch Linux containers.

### The build stops at pg_query

```text
fatal error: 'stdbool.h' file not found
...
Unable to generate bindings
```

bindgen found libclang but not the headers that come with clang: clang is not installed, or
its version is not the one of libclang. Install clang and libclang together with the command
for your distribution above, which installs the same version of both, then build again.

## Requirements

- PostgreSQL 12 or newer for a table's structure, its DDL, the key marks and copying rows as
  `INSERT`; MySQL 8.0 or newer; MariaDB 10.6 or newer (a best effort). See
  [Introduction](introduction.md#status) and [MySQL](guide/mysql.md).
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
stable releases.

See [What's new](whats-new.md) for the changes in each release. When a newer release is out,
datarig says so at its next start, with how to update your install (the Homebrew command, the
commands for a build from the source, or the download page); see [Updates](guide/updates.md),
which also says how to turn the check off.
