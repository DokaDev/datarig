#!/usr/bin/env bash
# Builds the docs site (docs/site) into docs/site/book and checks it, for the `docs` job of
# ci.yml and the `pages` job of release.yml (the same build is checked and deployed). Needs
# mdbook and lychee on the PATH (.github/scripts/docs-tools.sh).
#   1. `mdbook build docs/site`.
#   2. No `{{#...}}` left in the HTML: mdBook leaves an include whose file is missing as its
#      text and still succeeds (an unknown anchor includes nothing: tests/book_refs.rs checks
#      those).
#   3. Every link and anchor of the HTML, the README and CONTRIBUTING, offline with lychee
#      (external links are not fetched). mdBook's 404 page links to /datarig/..., so root-relative
#      links resolve in a directory where the build is datarig/.
set -euo pipefail
cd "$(dirname "$0")/../.."

mdbook build docs/site

if grep -rlF '{{#' docs/site/book --include='*.html'; then
  echo "an {{#...}} of mdBook was not resolved in the pages above" >&2
  exit 1
fi

site=$(mktemp -d)
trap 'rm -rf "$site"' EXIT
ln -s "$PWD/docs/site/book" "$site/datarig"
lychee --offline --include-fragments --no-progress --root-dir "$site" \
  docs/site/book README.md CONTRIBUTING.md
