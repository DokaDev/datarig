#!/bin/sh
# Writes THIRD-PARTY-NOTICES.html: the licenses of every crate built into the `datarig` binary
# (the dependency tree of crates/datarig-tui, through cargo-about with about.toml and about.hbs),
# followed by the notices of the C code pg_query compiles in (vendor/licenses/pg_query).
# Needs cargo-about: `cargo install --locked cargo-about --features cli`.
# Usage: scripts/third-party-notices.sh [output file]
set -eu
cd "$(dirname "$0")/.."
out=${1:-THIRD-PARTY-NOTICES.html}
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
cargo about generate --manifest-path crates/datarig-tui/Cargo.toml --locked --fail about.hbs > "$tmp"
{
    sed '/<\/body>/,$d' "$tmp"
    echo '<h2>C code built in through pg_query</h2>'
    echo '<p>libpg_query (BSD-3-Clause), the PostgreSQL parser it contains (PostgreSQL License),'
    echo 'protobuf-c and xxHash (BSD-2-Clause), and the pg_query crate itself (MIT).</p>'
    for f in vendor/licenses/pg_query/*; do
        printf '<h3>%s</h3>\n<pre>' "$(basename "$f")"
        sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' "$f"
        echo '</pre>'
    done
    echo '</body>'
    echo '</html>'
} > "$out"
echo "wrote $out"
