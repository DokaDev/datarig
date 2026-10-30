## What and why



<!--
The docs rule (CONTRIBUTING.md, "Documentation and changelog") may need one of these lines,
each on its own line outside this comment, with the real reason:
no doc change: <why no page of docs/site changes>
no changelog: <why users will not notice>
-->

- [ ] A page of `docs/site/src` describes what users see change (or `no doc change: …`)
- [ ] A fragment `changes/<kind>/<slug>.md` for a change to `crates/` or `locales/` (or
      `no changelog: …`)
- [ ] Generated docs regenerated with `DATARIG_BLESS=1 cargo test -p datarig-tui --test <test>`
      (`keybindings_doc`, `themes_doc`, `config_doc`, `cli_doc`, `book_screens`)
