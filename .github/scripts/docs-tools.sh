#!/usr/bin/env bash
# The tools that build and check the docs site (docs/site): mdBook and the lychee link checker,
# as their release binaries for x86_64 Linux (seconds, where `cargo install` takes minutes).
# Each is pinned to a version and to the SHA-256 of its archive (mdBook's from the GitHub
# release's asset digest, lychee's from its .sha256 file); a download that does not match fails
# here. They are CI tools only, never dependencies of datarig. Installed in $RUNNER_TEMP/bin,
# which goes on the PATH of the steps after this one. The archives are kept in
# ~/.cache/ci-downloads/docs-tools, which the workflows cache under a key made from this file
# (fetch.sh: checked again, downloaded only when missing, with retries).
# To bump one: change its version and its digest together.
set -euo pipefail

mdbook_version=v0.5.4
mdbook_sha256=3f28de05dafca9d0f2eab99c662116b0e37b89b1d96a08f8f430b9eeae958cd7
lychee_version=lychee-v0.24.2
lychee_sha256=1f4e0ef7f6554a6ed33dd7ac144fb2e1bbed98598e7af973042fc5cd43951c9a

if [ "$(uname -sm)" != "Linux x86_64" ]; then
  echo "docs-tools.sh downloads the x86_64 Linux builds; this is $(uname -sm)" >&2
  exit 1
fi

bin=${RUNNER_TEMP:?}/bin
downloads=$HOME/.cache/ci-downloads/docs-tools
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$bin"
# shellcheck source-path=SCRIPTDIR source=fetch.sh
. "$(dirname "$0")/fetch.sh"

fetch "https://github.com/rust-lang/mdBook/releases/download/$mdbook_version/mdbook-$mdbook_version-x86_64-unknown-linux-gnu.tar.gz" \
  "$mdbook_sha256" "$downloads"
tar -xzf "$downloads/mdbook-$mdbook_version-x86_64-unknown-linux-gnu.tar.gz" -C "$bin" mdbook

fetch "https://github.com/lycheeverse/lychee/releases/download/$lychee_version/lychee-x86_64-unknown-linux-gnu.tar.gz" \
  "$lychee_sha256" "$downloads"
tar -xzf "$downloads/lychee-x86_64-unknown-linux-gnu.tar.gz" -C "$work"
install -m 0755 "$work/lychee-x86_64-unknown-linux-gnu/lychee" "$bin/lychee"

if [ -n "${GITHUB_PATH:-}" ]; then
  echo "$bin" >> "$GITHUB_PATH"
fi
"$bin/mdbook" --version
"$bin/lychee" --version
