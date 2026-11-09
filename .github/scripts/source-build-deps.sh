#!/usr/bin/env bash
# The build dependencies of a source build on Linux, installed with the very command the docs
# give for this distribution (docs/site/src/install.md, "From source": the line that ends with
# `# Debian, Ubuntu`, `# Fedora` or `# Arch Linux`), so the page and what works cannot drift
# apart. Runs as root in a clean container of that distribution (source-build.yml): `sudo` is
# dropped and the package manager is told not to ask. Before it, only what a container lacks and
# a real system has: fresh package lists, and curl with CA certificates for rustup.
set -euo pipefail

page=${1:-docs/site/src/install.md}
# shellcheck source=/dev/null
. /etc/os-release
case "$ID" in
  debian | ubuntu) label='Debian, Ubuntu' ;;
  fedora) label='Fedora' ;;
  arch) label='Arch Linux' ;;
  *) echo "no install command in $page for $ID" >&2; exit 1 ;;
esac

# The line of the page for this distribution, without its comment and its indentation.
line=$(grep -E "^ *sudo .*# $label\$" "$page" | sed -E 's/^ *//; s/ *#.*$//')
if [[ $(grep -c . <<<"$line") != 1 ]]; then
  echo "expected one line ending with '# $label' in $page, found: ${line:-none}" >&2
  exit 1
fi
read -ra cmd <<<"${line#sudo }"

case "$ID" in
  debian | ubuntu)
    export DEBIAN_FRONTEND=noninteractive
    apt-get -o Acquire::Retries=5 update
    apt-get -o Acquire::Retries=5 install -y --no-install-recommends ca-certificates curl
    [[ ${cmd[0]} == apt ]] || { echo "not an apt command: $line" >&2; exit 1; }
    cmd+=(-y -o Acquire::Retries=5)
    ;;
  fedora)
    dnf install -y --setopt=retries=10 curl
    [[ ${cmd[0]} == dnf ]] || { echo "not a dnf command: $line" >&2; exit 1; }
    cmd+=(-y --setopt=retries=10)
    ;;
  arch)
    pacman -Sy --noconfirm --needed curl ca-certificates
    [[ ${cmd[0]} == pacman ]] || { echo "not a pacman command: $line" >&2; exit 1; }
    cmd+=(--noconfirm)
    ;;
esac

echo "+ ${cmd[*]}   (from $page)"
"${cmd[@]}"
