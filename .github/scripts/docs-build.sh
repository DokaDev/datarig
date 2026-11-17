#!/usr/bin/env bash
# Builds the docs site (docs/site) into docs/site/book and checks it, for the `docs` job of
# ci.yml and the `pages` job of release.yml (the same build is checked and deployed). Needs
# mdbook and lychee on the PATH (.github/scripts/docs-tools.sh).
#   1. `mdbook build docs/site`.
#   2. No `{{#...}}` left in the HTML: mdBook leaves an include whose file is missing as its
#      text and still succeeds (an unknown anchor includes nothing: tests/book_refs.rs checks
#      those).
#   3. Each page's own description, from docs/site/descriptions.txt, and sitemap.xml.
#   4. Every link and anchor of the HTML, the README and CONTRIBUTING, offline with lychee
#      (external links are not fetched). mdBook's 404 page links to /datarig/..., so root-relative
#      links resolve in a directory where the build is datarig/.
set -euo pipefail
cd "$(dirname "$0")/../.."

mdbook build docs/site

if grep -rlF '{{#' docs/site/book --include='*.html'; then
  echo "an {{#...}} of mdBook was not resolved in the pages above" >&2
  exit 1
fi

# Each page gets its own description from docs/site/descriptions.txt (mdBook gives every page
# the book's), and sitemap.xml lists the pages. A page of the build without a line there, or a
# line without a page, fails here, so the two cannot drift apart.
python3 - <<'EOF'
import html, pathlib, re, sys

book = pathlib.Path("docs/site/book")
table = pathlib.Path("docs/site/descriptions.txt")
base = "https://dokadev.github.io/datarig/"
# Pages of mdBook's own that search results should not show.
skip = {"404.html", "print.html", "toc.html"}

descriptions = {}
for n, line in enumerate(table.read_text(encoding="utf-8").splitlines(), 1):
    if not line.strip() or line.startswith("#"):
        continue
    path, _, text = line.partition(" ")
    if not text.strip() or path in descriptions:
        sys.exit(f"{table}:{n}: want a page path, a space and its description, once per page")
    descriptions[path] = text.strip()
descriptions["index.html"] = descriptions.get("introduction.html", "")

pages = sorted(p.relative_to(book).as_posix() for p in book.rglob("*.html"))
pages = [p for p in pages if p not in skip]
errors = [f"{p}: no line in {table}" for p in pages if not descriptions.get(p)]
errors += [f"{table}: {p} is not a page of the build" for p in descriptions if p not in pages]
meta = re.compile(r'<meta name="description" content="[^"]*">')
for p in pages:
    if p not in descriptions:
        continue
    f = book / p
    text, count = meta.subn(
        lambda _: f'<meta name="description" content="{html.escape(descriptions[p])}">',
        f.read_text(encoding="utf-8"),
    )
    if count != 1:
        errors.append(f"{p}: {count} description tags, want 1")
    f.write_text(text, encoding="utf-8")
if errors:
    sys.exit("\n".join(errors))

# The home page once, at the site's root (index.html is introduction.html).
urls = [base] + [base + p for p in pages if p not in ("index.html", "introduction.html")]
(book / "sitemap.xml").write_text(
    '<?xml version="1.0" encoding="UTF-8"?>\n'
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
    + "".join(f"  <url><loc>{html.escape(u)}</loc></url>\n" for u in urls)
    + "</urlset>\n",
    encoding="utf-8",
)
EOF

site=$(mktemp -d)
trap 'rm -rf "$site"' EXIT
ln -s "$PWD/docs/site/book" "$site/datarig"
lychee --offline --include-fragments --no-progress --root-dir "$site" \
  docs/site/book README.md CONTRIBUTING.md
