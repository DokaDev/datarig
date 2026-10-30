# Changelog fragments

Each pull request that changes `crates/` or `locales/` adds one file here saying what changed
for users. At release time they become the version's section of
[What's new](../docs/site/src/whats-new.md), which is also the top of the GitHub release notes.
A pull request users will not notice says so in its description instead, on a line of its own:

```text
no changelog: <why>
```

## Format

`changes/<kind>/<slug>.md`:

- `<kind>` is one of `added`, `changed`, `fixed`, `removed`, `security`.
- `<slug>` is lowercase letters, digits and dashes: `grid-copy-tsv.md`.
- The content is one or more bullets in English, written for users. A bullet may continue on
  the next lines, indented by two spaces. No headings, no blank lines.

For example `changes/added/grid-copy-tsv.md`:

```markdown
- The result grid copies a selected range as tab-separated values, ready to paste into a
  spreadsheet.
```

A change of the configuration formats always has a `changed` fragment that says so: what
changed, and that datarig migrates the old files (keeping a backup), so that What's new and
the release notes tell users before they upgrade.

Check fragments with `scripts/changes.sh check changes/*/*.md`; the docs rule of pull requests
checks them too.

## Releasing

In the release pull request, after setting the version:

```sh
scripts/changes.sh release 0.37.0
```

writes the fragments as a `## 0.37.0 — <date>` section at the top of What's new (grouped
Added, Changed, Fixed, Removed, Security) and deletes them; commit both. A release without
fragments, such as an urgent patch, gives the one line its section says:

```sh
scripts/changes.sh release 0.36.1 --note "Fix the crash when a query returns no columns."
```
