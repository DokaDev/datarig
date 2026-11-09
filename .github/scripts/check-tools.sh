#!/usr/bin/env bash
# check-tools.sh <tool>: one of the checkers CI runs, as its release binary for x86_64 Linux:
# `actionlint` (the workflows) or `cargo-deny` (deny.toml). Each is pinned to a version and to
# the SHA-256 of its archive (from the GitHub release's asset digest); a download that does not
# match fails here. CI tools only, never dependencies of datarig. Installed in $RUNNER_TEMP/bin,
# which goes on the PATH of the steps after this one. The archives are kept in
# ~/.cache/ci-downloads/check-tools, which ci.yml caches under a key made from this file
# (fetch.sh: checked again, downloaded only when missing, with retries).
# To bump one: change its version and its digest together.
set -euo pipefail

actionlint_version=1.7.12
actionlint_sha256=8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8
cargo_deny_version=0.20.2
cargo_deny_sha256=9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f

if [ "$(uname -sm)" != "Linux x86_64" ]; then
  echo "check-tools.sh downloads the x86_64 Linux builds; this is $(uname -sm)" >&2
  exit 1
fi

bin=${RUNNER_TEMP:?}/bin
downloads=$HOME/.cache/ci-downloads/check-tools
mkdir -p "$bin"
# shellcheck source-path=SCRIPTDIR source=fetch.sh
. "$(dirname "$0")/fetch.sh"

case "${1:-}" in
  actionlint)
    fetch "https://github.com/rhysd/actionlint/releases/download/v$actionlint_version/actionlint_${actionlint_version}_linux_amd64.tar.gz" \
      "$actionlint_sha256" "$downloads"
    tar -xzf "$downloads/actionlint_${actionlint_version}_linux_amd64.tar.gz" -C "$bin" actionlint
    "$bin/actionlint" -version
    ;;
  cargo-deny)
    name=cargo-deny-$cargo_deny_version-x86_64-unknown-linux-musl
    fetch "https://github.com/EmbarkStudios/cargo-deny/releases/download/$cargo_deny_version/$name.tar.gz" \
      "$cargo_deny_sha256" "$downloads"
    tar -xzf "$downloads/$name.tar.gz" -C "$bin" --strip-components=1 "$name/cargo-deny"
    "$bin/cargo-deny" --version
    ;;
  *)
    echo "usage: check-tools.sh actionlint|cargo-deny" >&2
    exit 2
    ;;
esac

if [ -n "${GITHUB_PATH:-}" ]; then
  echo "$bin" >> "$GITHUB_PATH"
fi
