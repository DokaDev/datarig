#!/usr/bin/env bash
# Tests of .github/scripts/code-changed.sh, against a scratch git repository with a pull request
# per case: bash .github/scripts/tests/code-changed-test.sh
set -euo pipefail

root=$(cd "$(dirname "$0")/../../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
repo=$work/repo
failures=0

g() { git -C "$repo" "$@"; }

put() { # put <path> <content>
  mkdir -p "$(dirname "$repo/$1")"
  printf '%s' "$2" > "$repo/$1"
}

mkdir -p "$repo"
g init -q -b main
g config user.name test
g config user.email test@example.com
g config commit.gpgsign false
put README.md $'# datarig\n'
put docs/site/src/guide.md $'# Guide\n'
put crates/core/src/lib.rs $'// core\n'
put changes/README.md $'# Fragments\n'
g add -A
g commit -q -m base

# case <name> <expected code> <commands>: on a branch off main, run the commands (one string),
# commit, and check what the script says for main...branch.
case_() {
  local name=$1 want=$2 commands=$3
  g checkout -q -b "$name" main
  (cd "$repo" && eval "$commands")
  g add -A
  g commit -q --allow-empty -m "$name"
  local got
  got=$(cd "$repo" && BASE=$(g rev-parse main) HEAD=$(g rev-parse HEAD) GITHUB_OUTPUT=/dev/stdout \
    bash "$root/.github/scripts/code-changed.sh" | sed -n 's/^code=//p')
  if [[ $got == "$want" ]]; then
    echo "ok   $name"
  else
    echo "FAIL $name: code=$got, want $want"
    failures=$((failures + 1))
  fi
}

case_ readme false "echo more >> README.md"
case_ contributing false "echo x > CONTRIBUTING.md"
case_ site false "mkdir -p docs/site/src/reference && echo x > docs/site/src/reference/keys.md"
case_ fragment false "mkdir -p changes/fixed && echo '- x' > changes/fixed/a-b.md"
case_ code true "echo '// more' >> crates/core/src/lib.rs"
case_ docs_and_code true "echo more >> README.md && echo '// more' >> crates/core/src/lib.rs"
case_ fragment_not_md true "mkdir -p changes/fixed && echo x > changes/fixed/a.txt"
case_ readme_elsewhere true "mkdir -p crates/core && echo x > crates/core/README.md"
case_ moved_out_of_code true "git mv crates/core/src/lib.rs docs/lib.rs"
case_ nothing true "true"

# check <name> <got>: everything runs.
check() {
  if [[ $2 == true ]]; then
    echo "ok   $1"
  else
    echo "FAIL $1: code=$2, want true"
    failures=$((failures + 1))
  fi
}

# Not a pull request, and a diff that cannot be read: everything runs.
check push "$(cd "$repo" && GITHUB_OUTPUT=/dev/stdout bash "$root/.github/scripts/code-changed.sh" | sed -n 's/^code=//p')"
check unknown_base "$(cd "$repo" && BASE=0000000000000000000000000000000000000000 HEAD=$(g rev-parse HEAD) \
  GITHUB_OUTPUT=/dev/stdout bash "$root/.github/scripts/code-changed.sh" 2> /dev/null | sed -n 's/^code=//p')"

if ((failures > 0)); then
  echo "$failures failed"
  exit 1
fi
echo "all passed"
