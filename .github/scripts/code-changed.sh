#!/usr/bin/env bash
# Whether a pull request changes more than prose (.github/workflows/ci.yml, job `changes`): its
# files (`git diff --no-renames BASE...HEAD`, both sides of a move) are only README.md,
# CONTRIBUTING.md, files under docs/, or changelog fragments changes/**/*.md. Writes `code=false`
# for such a pull request and `code=true` for any other, to $GITHUB_OUTPUT (stdout when unset).
# Anything it cannot tell (no BASE or HEAD, a diff that fails, no file at all) is `code=true`:
# the full run is never skipped on a guess. Reads only git objects, never the working tree.
set -euo pipefail

out=${GITHUB_OUTPUT:-/dev/stdout}
summary=${GITHUB_STEP_SUMMARY:-/dev/null}

verdict() { # verdict <true|false> <why>
  echo "code=$1" >> "$out"
  echo "$2" >> "$summary"
  echo "$2"
  exit 0
}

if [[ -z ${BASE:-} || -z ${HEAD:-} ]]; then
  verdict true "Not a pull request: everything runs."
fi
if ! files=$(git diff --no-renames --name-only "$BASE...$HEAD"); then
  verdict true "The changed files could not be read: everything runs."
fi
if [[ -z $files ]]; then
  verdict true "No changed file: everything runs."
fi
while IFS= read -r f; do
  case $f in
    README.md | CONTRIBUTING.md | docs/*) ;;
    changes/*.md) ;;
    *) verdict true "Changes $f: everything runs." ;;
  esac
done <<< "$files"
verdict false "Docs only (README.md, CONTRIBUTING.md, docs/, changes/*.md): the builds, tests, \
clippy, integration and performance jobs are skipped; the docs site and the pages checked \
against the code still run."
