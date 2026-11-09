Related issue: Part of #

<!--
Name the issue with "Part of #123", not with Closes or Fixes: the maintainers decide when an
issue is done. A small fix without an issue (a typo, a broken link) says why instead, on its
own line outside this comment: no issue: <reason>
A release pull request says: no issue: release
-->

## What and why



<!--
The docs rule (CONTRIBUTING.md, "Documentation and changelog") may need one of these lines,
each on its own line outside this comment, with the real reason:
no doc change: <why no page of docs/site changes>
no changelog: <why users will not notice>
-->

- [ ] This change was discussed and assigned in its issue (or is a small fix such as a typo)
- [ ] A page of `docs/site/src` describes what users see change (or `no doc change: …`)
- [ ] A fragment `changes/<kind>/<slug>.md` for a change to `crates/` (or
      `no changelog: …`)
- [ ] Generated docs regenerated with `DATARIG_BLESS=1 cargo test -p datarig-tui --test <test>`
      (`keybindings_doc`, `themes_doc`, `config_doc`, `cli_doc`, `book_screens`)
