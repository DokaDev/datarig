use super::*;
use crate::app::action::{Action, ExplorerAction, spec};
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use std::collections::BTreeMap;

fn k(s: &str) -> Vec<KeyChord> {
    parse_keys(s).unwrap_or_else(|e| panic!("{s}: {e}"))
}

fn feed_all(km: &Keymap, ctx: Ctx, keys: &str) -> Vec<Resolved> {
    let mut st = KeyState::default();
    k(keys).into_iter().map(|c| km.feed(&mut st, ctx, c)).collect()
}

/// The result of the last key of `keys`.
fn last(km: &Keymap, ctx: Ctx, keys: &str) -> Resolved {
    feed_all(km, ctx, keys).pop().unwrap()
}

fn act(id: &str) -> Resolved {
    Resolved::Action(action::by_id(id).unwrap_or_else(|| panic!("no action {id}")).action)
}

// ── key notation ─────────────────────────────────────────────────────────────

#[test]
fn key_notation_round_trips() {
    for (notation, label) in [
        ("ctrl+e", "Ctrl+E"),
        ("ctrl+enter", "Ctrl+Enter"),
        ("shift+tab", "Shift+Tab"),
        ("shift+f6", "Shift+F6"),
        ("f4", "F4"),
        ("pagedown", "PageDown"),
        ("ctrl+pageup", "Ctrl+PageUp"),
        ("space c n", "Space c n"),
        ("g t", "g t"),
        ("G", "G"),
        ("?", "?"),
        ("$", "$"),
        ("alt+x", "Alt+x"),
        ("shift+left", "Shift+Left"),
        ("ctrl+backspace", "Ctrl+Backspace"),
    ] {
        let keys = k(notation);
        assert_eq!(keys::label(&keys), label, "{notation}");
        assert_eq!(keys::notation(&keys), notation, "{notation}");
        assert_eq!(parse_keys(&keys::notation(&keys)).unwrap(), keys);
    }
    // Same key, different spellings.
    assert_eq!(k("shift+g"), k("G"));
    assert_eq!(k("Ctrl+E"), k("ctrl+e"));
    assert_eq!(k("ctrl+shift+e"), k("ctrl+e"), "terminals cannot tell them apart");
    assert_eq!(k("escape"), k("esc"));
    assert_eq!(k("pgdn"), k("pagedown"));
    assert_eq!(k("  g   t "), k("g t"));
    for bad in ["", "ctrl+", "hyper+x", "foo", "shift+?", "f99"] {
        assert!(parse_keys(bad).is_err(), "{bad:?} must not parse");
    }
}

#[test]
fn key_events_are_normalized() {
    let ev = |code, m| KeyChord::new(code, m);
    assert_eq!(ev(KeyCode::Char('g'), KeyModifiers::SHIFT), KeyChord::char('G'), "kitty may send g + Shift");
    assert_eq!(ev(KeyCode::Char('G'), KeyModifiers::SHIFT), KeyChord::char('G'));
    assert_eq!(ev(KeyCode::Char('?'), KeyModifiers::SHIFT), KeyChord::char('?'));
    assert_eq!(ev(KeyCode::Char('E'), KeyModifiers::CONTROL), k("ctrl+e")[0]);
    assert_eq!(ev(KeyCode::Tab, KeyModifiers::SHIFT), k("shift+tab")[0]);
    assert_eq!(ev(KeyCode::BackTab, KeyModifiers::SHIFT), k("shift+tab")[0]);
    assert!(KeyChord::char('j').is_plain_char() && !k("ctrl+j")[0].is_plain_char() && !k("f4")[0].is_plain_char());
    assert_eq!(k("ctrl+e")[0].to_event().modifiers, KeyModifiers::CONTROL);
}

// ── contexts ─────────────────────────────────────────────────────────────────

#[test]
fn contexts_form_the_planned_tree() {
    for c in Ctx::ALL {
        assert_eq!(Ctx::from_name(c.name()), Some(c));
        assert_eq!(c.chain().last(), Some(&Ctx::Root));
    }
    assert_eq!(Ctx::VimNormal.chain(), [Ctx::VimNormal, Ctx::Nav, Ctx::Workspace, Ctx::Root]);
    assert_eq!(Ctx::VimInsert.chain(), [Ctx::VimInsert, Ctx::Workspace, Ctx::Root]);
    assert_eq!(Ctx::Commands.chain(), [Ctx::Commands, Ctx::Root], "dialogs only see root keys");
    assert_eq!(Ctx::CellViewer.chain(), [Ctx::CellViewer, Ctx::Workspace, Ctx::Root], "not modal");
    assert_eq!(Ctx::HelpFilter.chain(), [Ctx::HelpFilter, Ctx::Root]);
    assert!(Ctx::HelpFilter.is_text_input() && !Ctx::Help.is_text_input() && !Ctx::WhichKey.is_text_input());
    assert!(Ctx::VimInsert.is_text_input() && Ctx::VimInsert.is_editor());
    assert!(!Ctx::VimNormal.is_text_input() && Ctx::VimNormal.is_editor());
    assert!(!Ctx::Explorer.is_editor() && !Ctx::Explorer.is_text_input());
}

// ── the default table ────────────────────────────────────────────────────────

#[test]
fn default_table_is_conflict_free() {
    let raw = defaults();
    assert_eq!(check_ids(&raw), Vec::<String>::new(), "every default parses and names a real action");
    let km = Keymap::default();
    assert_eq!(km.bindings().len(), raw.len());
    let conflicts: Vec<String> = check_contexts(km.bindings(), &Ctx::ALL).iter().map(ToString::to_string).collect();
    assert!(conflicts.is_empty(), "{}", conflicts.join("\n"));
    // Every action has at least one key or is reachable from the command line only on purpose.
    let commands_only = [
        "explorer.context_menu",
        "help.all",
        "ui.icons.on",
        "ui.icons.off",
        // `Space ,` opens the settings screen, where these are changed.
        "ui.language.en",
        "ui.language.ko",
        "ui.language.auto",
        "ui.icons.toggle",
        "ui.icons.auto",
        "secrets.default.auto",
        "secrets.default.keychain",
        "secrets.default.file",
        "secrets.default.command",
        "secrets.default.env",
        "secrets.default.prompt",
    ];
    for s in action::REGISTRY {
        let bound = km.bindings().iter().any(|b| b.target == Target::Action(s.action));
        assert!(bound || commands_only.contains(&s.id), "{} has no key", s.id);
    }
}

fn bound(ctx: Ctx, keys: &str, target: Target) -> Bound {
    Bound { ctx, keys: k(keys), target, user: false }
}

fn with(extra: Vec<Bound>) -> Vec<Bound> {
    let mut v = Keymap::default().bindings().to_vec();
    v.extend(extra);
    v
}

fn kinds(b: &[Bound]) -> Vec<ConflictKind> {
    let mut v: Vec<ConflictKind> = check(b).iter().map(|c| c.kind).collect();
    v.sort();
    v.dedup();
    v
}

#[test]
fn conflict_check_catches_all_seven_kinds() {
    let refresh = Target::Action(spec(Action::Explorer(ExplorerAction::Refresh)).action);
    let res = Target::Reserved("test");
    // 1. the same keys twice in one context
    let dup = with(vec![bound(Ctx::Explorer, "x", refresh), bound(Ctx::Explorer, "x", refresh)]);
    assert_eq!(kinds(&dup), [ConflictKind::Duplicate]);
    // 2. a pane hides an ancestor's key (`:` opens the command line in every non-text pane)
    assert_eq!(kinds(&with(vec![bound(Ctx::Explorer, ":", refresh)])), [ConflictKind::Shadow]);
    // … which the editor may do
    assert!(kinds(&with(vec![bound(Ctx::VimNormal, ":", res)])).is_empty());
    // … and the inspector only with its own tab switch over the pane keys
    let tab_refresh = check(&with(vec![bound(Ctx::Inspector, "tab", refresh)]));
    assert!(
        tab_refresh.iter().any(|c| c.kind == ConflictKind::Shadow && c.a.target == refresh),
        "another action on Tab there hides the pane keys: {tab_refresh:?}"
    );
    assert_eq!(kinds(&with(vec![bound(Ctx::Inspector, ":", refresh)])), [ConflictKind::Shadow]);
    // 3. a sequence is a prefix of another (`g` alone vs `g g`, `g t`)
    let prefix = check(&with(vec![bound(Ctx::Explorer, "g", refresh)]));
    assert!(prefix.iter().all(|c| c.kind == ConflictKind::Prefix) && !prefix.is_empty(), "{prefix:?}");
    // 4. an app binding on a reserved key (`d` is vim's delete in the editor)
    let refresh_in_editor = with(vec![bound(Ctx::VimNormal, "d", refresh)]);
    assert_eq!(kinds(&refresh_in_editor), [ConflictKind::Reserved]);
    // 5. an unknown action id in the raw table
    let raw = [Binding { ctx: Ctx::Explorer, keys: "w", target: BindTarget::Action("no.such.action") }];
    let ids = check_ids(&raw);
    assert!(ids.len() == 1 && ids[0].contains("UnknownAction") && ids[0].contains("no.such.action"), "{ids:?}");
    // 6. a character or a leader sequence in a text input context
    assert_eq!(kinds(&with(vec![bound(Ctx::Commands, "a", res)])), [ConflictKind::TextKey]);
    assert_eq!(kinds(&with(vec![bound(Ctx::VimInsert, "space x", res)])), [ConflictKind::TextKey]);
    // 7. hiding a protected key, even from the editor
    assert_eq!(kinds(&with(vec![bound(Ctx::VimNormal, "ctrl+e", res)])), [ConflictKind::Protected]);
    assert_eq!(kinds(&with(vec![bound(Ctx::VimNormal, "space", res)])), [ConflictKind::Protected], "the leader");
    assert_eq!(kinds(&with(vec![bound(Ctx::VimInsert, "f1", res)])), [ConflictKind::Protected]);
    // The message names the context, the keys and both sides.
    let msg = check(&dup)[0].to_string();
    assert!(msg.contains("explorer") && msg.contains("x = explorer.refresh"), "{msg}");
}

// ── resolving keys ───────────────────────────────────────────────────────────

#[test]
fn innermost_context_wins_and_sequences_complete() {
    let km = Keymap::default();
    assert_eq!(last(&km, Ctx::Explorer, "j"), act("explorer.down"));
    assert_eq!(last(&km, Ctx::Explorer, ":"), act("commands.open"), "from nav");
    assert_eq!(last(&km, Ctx::Explorer, "ctrl+e"), act("query.execute_current"), "from workspace");
    assert_eq!(last(&km, Ctx::Explorer, "ctrl+q"), act("app.quit"), "from root");
    assert_eq!(feed_all(&km, Ctx::Explorer, "g g"), [Resolved::Pending, act("explorer.top")]);
    assert_eq!(feed_all(&km, Ctx::Grid, "g g"), [Resolved::Pending, act("grid.top")]);
    assert_eq!(feed_all(&km, Ctx::Explorer, "space c n"), [Resolved::Pending, Resolved::Pending, act("conn.new")]);
    assert_eq!(last(&km, Ctx::Explorer, "space r a c"), act("results.copy.all.csv"));
    assert_eq!(last(&km, Ctx::Grid, "space r y J"), act("results.copy.selection.json_pretty"));
    assert_eq!(last(&km, Ctx::Explorer, "space ,"), act("settings.open"));
    assert_eq!(last(&km, Ctx::Explorer, "g t"), act("tab.next"));
    assert_eq!(last(&km, Ctx::Explorer, "space 3"), act("tab.goto.3"));
    // A reserved key goes to the widget as typed.
    assert_eq!(last(&km, Ctx::VimNormal, "d"), Resolved::Forward(k("d")));
    // Unknown keys go to the widget; an unfinished leader sequence is dropped.
    assert_eq!(last(&km, Ctx::Explorer, "z"), Resolved::Forward(k("z")));
    assert_eq!(last(&km, Ctx::Explorer, "g x"), Resolved::Forward(k("g x")));
    assert_eq!(last(&km, Ctx::Explorer, "space x"), Resolved::Unbound(k("space x")));
    assert_eq!(last(&km, Ctx::Explorer, "space esc"), Resolved::Unbound(k("space esc")));
    // Switching context drops a pending sequence.
    let mut st = KeyState::default();
    assert_eq!(km.feed(&mut st, Ctx::Explorer, KeyChord::char('g')), Resolved::Pending);
    assert_eq!(st.pending(), k("g"));
    assert_eq!(km.feed(&mut st, Ctx::VimInsert, KeyChord::char('g')), Resolved::Forward(k("g")));
    assert!(st.pending().is_empty());
}

#[test]
fn editor_keys_win_inside_the_editor_except_protected_keys() {
    let km = Keymap::default();
    // vim Normal: `?` is backward search (reserved), help is Space ? or F1.
    assert_eq!(last(&km, Ctx::VimNormal, "?"), Resolved::Forward(k("?")));
    assert_eq!(last(&km, Ctx::VimNormal, "space ?"), act("help.context"));
    assert_eq!(last(&km, Ctx::VimNormal, "f1"), act("help.context"));
    assert_eq!(last(&km, Ctx::VimNormal, "]"), Resolved::Forward(k("]")), "bracket motion, not a tab key");
    // `g g` is the editor's; `g t` is the tab key; both work.
    assert_eq!(feed_all(&km, Ctx::VimNormal, "g g"), [Resolved::Pending, Resolved::Forward(k("g g"))]);
    assert_eq!(last(&km, Ctx::VimNormal, "g t"), act("tab.next"));
    assert_eq!(last(&km, Ctx::VimNormal, "g T"), act("tab.prev"));
    assert_eq!(last(&km, Ctx::VimNormal, "q"), Resolved::Forward(k("q")), "q no longer quits from the editor");
    // Protected keys work in every mode.
    for ctx in [Ctx::VimNormal, Ctx::VimVisual, Ctx::VimInsert] {
        assert_eq!(last(&km, ctx, "ctrl+e"), act("query.execute_current"), "{ctx:?}");
        assert_eq!(last(&km, ctx, "ctrl+k"), act("commands.open"), "{ctx:?}");
        assert_eq!(last(&km, ctx, "f1"), act("help.context"), "{ctx:?}");
    }
    // vim Insert: Ctrl+W deletes a word (reserved), it never closes a tab; elsewhere it does.
    assert_eq!(last(&km, Ctx::VimInsert, "ctrl+w"), Resolved::Forward(k("ctrl+w")));
    assert_eq!(last(&km, Ctx::VimNormal, "ctrl+w"), act("tab.close"));
    assert_eq!(last(&km, Ctx::Explorer, "ctrl+w"), act("tab.close"));
    // Tab keys that work in every editor mode (protected).
    for ctx in [Ctx::VimNormal, Ctx::VimVisual, Ctx::VimInsert] {
        assert_eq!(last(&km, ctx, "ctrl+t"), act("tab.new_console"), "{ctx:?}");
        assert_eq!(last(&km, ctx, "ctrl+pagedown"), act("tab.next"), "{ctx:?}");
        assert_eq!(last(&km, ctx, "f6"), act("pane.next"), "{ctx:?}");
    }
    assert_eq!(last(&km, Ctx::VimInsert, "ctrl+n"), act("editor.complete"));
    assert_eq!(last(&km, Ctx::VimInsert, "shift+tab"), act("pane.prev"));
}

/// `[` and `]` are no tab keys: in the editor they are vim's
/// motions, and elsewhere they are left free, so the same key never does two different things.
/// The result tabs keep `Space r [` / `Space r ]`.
#[test]
fn brackets_are_no_tab_keys() {
    let km = Keymap::default();
    for ctx in [Ctx::Explorer, Ctx::Grid, Ctx::Inspector, Ctx::Welcome] {
        assert_eq!(last(&km, ctx, "]"), Resolved::Forward(k("]")), "{ctx:?}");
        assert_eq!(last(&km, ctx, "["), Resolved::Forward(k("[")), "{ctx:?}");
    }
    assert_eq!(last(&km, Ctx::Explorer, "space r ]"), act("results.tab.next"));
    let tab_keys: Vec<String> =
        km.keys_for(spec(Action::NextTab).action, Ctx::Explorer).iter().map(|k| keys::notation(k)).collect();
    assert_eq!(tab_keys, ["ctrl+pagedown", "g t"]);
}

/// `Ctrl+C` cancels the query in every editor mode (it has no copy meaning), and the vim keys
/// the editor handles are reserved there, `D`, `C`, `Y`, `V` and `Ctrl+V` included, the motions,
/// operators, edits and scrolling keys of more than one stroke too.
#[test]
fn ctrl_c_cancels_everywhere_and_vim_keys_reach_the_editor() {
    let km = Keymap::default();
    for ctx in [Ctx::VimNormal, Ctx::VimVisual, Ctx::VimInsert] {
        assert_eq!(last(&km, ctx, "ctrl+c"), act("query.cancel"), "{ctx:?}");
    }
    for ctx in [Ctx::VimNormal, Ctx::VimVisual] {
        for key in [
            "V", "v", "o", "D", "C", "Y", "X", "3", "0", "g g", "esc", "W", "B", "E", "g e", "g E", "f", "F", "t", "T",
            ";", ",", "%", "{", "}", "H", "M", "L", "r", "J", "g J", "~", ".", ">", "<", "g u", "g U", "g ~", "ctrl+d",
            "ctrl+u", "ctrl+f", "ctrl+b", "z z", "z t", "z b", "z enter", "ctrl+v", "I", "A", "O",
        ] {
            assert_eq!(last(&km, ctx, key), Resolved::Forward(k(key)), "{ctx:?} {key}");
            assert!(km.bindings().iter().any(|b| b.ctx == ctx && b.keys == k(key)), "{key} reserved in {ctx:?}");
        }
    }
    for key in ["ctrl+w", "ctrl+u", "esc", "enter"] {
        assert_eq!(last(&km, Ctx::VimInsert, key), Resolved::Forward(k(key)), "{key}");
    }
    // While a command waits for its next key (`d…`, `f…`, `i(`) the keys are resolved in the
    // workspace: the leader, `:` and the brackets reach the editor as they are.
    for key in ["space", ":", "(", "}", "w", "j", "g", "G", "tab", "enter", "\""] {
        assert_eq!(last(&km, Ctx::Workspace, key), Resolved::Forward(k(key)), "{key}");
    }
}

#[test]
fn text_input_takes_letters_space_and_hangul_as_text() {
    let km = Keymap::default();
    for ctx in Ctx::ALL.into_iter().filter(|c| c.is_text_input()) {
        for key in ["j", "q", "space", "G", ":", "?"] {
            assert_eq!(last(&km, ctx, key), Resolved::Forward(k(key)), "{ctx:?} {key}");
        }
        let mut st = KeyState::default();
        assert_eq!(
            km.feed(&mut st, ctx, KeyChord::char('\u{3153}')),
            Resolved::Forward(vec![KeyChord::char('\u{3153}')]),
            "{ctx:?}"
        );
    }
}

#[test]
fn repeatable_marks_movement_only() {
    let rep = |id: &str| action::by_id(id).unwrap().repeatable;
    for id in ["explorer.down", "explorer.up", "grid.down", "grid.page_up", "grid.half_down"] {
        assert!(rep(id), "{id}");
    }
    for id in ["query.execute_current", "commands.open", "app.quit", "pane.next", "explorer.top", "conn.new"] {
        assert!(!rep(id), "{id}");
    }
}

#[test]
fn command_line_shows_the_keys_of_the_context() {
    let km = Keymap::default();
    let label = |id: &str, ctx| km.keys_label(action::by_id(id).unwrap().action, ctx);
    assert_eq!(label("query.execute_current", Ctx::VimNormal), "Ctrl+Enter / Ctrl+E");
    assert_eq!(label("commands.open", Ctx::VimNormal), "Ctrl+K / : / Space /");
    assert_eq!(label("commands.open", Ctx::VimInsert), "Ctrl+K", "no leader or `:` while typing");
    assert_eq!(label("help.context", Ctx::VimNormal), "F1 / Space ?");
    assert_eq!(label("help.context", Ctx::Explorer), "F1 / Space ?", "the same keys everywhere");
    assert_eq!(label("editor.complete", Ctx::VimNormal), "Ctrl+N / F4", "keys of the context that has it");
    assert_eq!(label("help.all", Ctx::VimNormal), "", "command line only");
}

// ── user remapping ───────────────────────────────────────────────────────────

fn user(entries: &[(&str, &str, &str)]) -> (Keymap, Vec<Issue>) {
    let mut cfg: KeymapConfig = BTreeMap::new();
    for (ctx, key, action) in entries {
        cfg.entry(ctx.to_string()).or_default().insert(key.to_string(), action.to_string());
    }
    Keymap::with_user(&cfg)
}

#[test]
fn user_entries_apply_over_the_defaults() {
    let (km, issues) = user(&[
        ("explorer", "w", "explorer.refresh"),
        ("explorer", "j", "explorer.up"),
        ("explorer", "q", "none"),
        ("explorer", "ctrl+w", "none"),
        ("nav", "space c m", "help.all"),
    ]);
    assert_eq!(last(&km, Ctx::Explorer, "w"), act("explorer.refresh"));
    assert_eq!(last(&km, Ctx::Explorer, "j"), act("explorer.up"), "user wins over the default");
    assert_eq!(last(&km, Ctx::Explorer, "down"), act("explorer.down"), "other keys of the action stay");
    assert_eq!(last(&km, Ctx::Explorer, "q"), Resolved::Unbound(k("q")), "unbound");
    assert_eq!(last(&km, Ctx::Grid, "q"), act("pane.back"), "only in that context");
    assert_eq!(last(&km, Ctx::Grid, "space c m"), act("help.all"));
    assert_eq!(
        issues,
        [Issue {
            ctx: "explorer".into(),
            key: "j".into(),
            kind: IssueKind::Replaced { action: "explorer.up".into(), old: "explorer.down".into() }
        }]
    );
    assert!(check(km.bindings()).iter().all(|c| c.kind == ConflictKind::Shadow || !c.a.user));
    assert_eq!(km.keys_label(Action::Explorer(ExplorerAction::Refresh), Ctx::Explorer), "r / w");
    assert_eq!(km.keys_label(Action::Explorer(ExplorerAction::Down), Ctx::Explorer), "Down");
}

#[test]
fn bad_user_entries_are_skipped_and_reported() {
    let (km, issues) = user(&[
        ("nowhere", "x", "explorer.refresh"),
        ("explorer", "hyper+x", "explorer.refresh"),
        ("explorer", "w", "no.such.action"),
        ("overlay.commands", "a", "commands.open"),
        ("editor.vim.insert", "space", "commands.open"),
        ("editor.vim.normal", "d", "explorer.refresh"),
        ("explorer", "ctrl+e", "explorer.refresh"),
        ("editor.vim.normal", "ctrl+e", "none"),
        ("explorer", "g", "explorer.top"),
        ("welcome", "enter", "app.quit"),
    ]);
    let got: Vec<(&str, &str, &IssueKind)> = issues.iter().map(|i| (i.ctx.as_str(), i.key.as_str(), &i.kind)).collect();
    assert!(got.contains(&("nowhere", "", &IssueKind::UnknownContext)), "{got:?}");
    assert!(matches!(got.iter().find(|g| g.1 == "hyper+x"), Some((_, _, IssueKind::BadKey(_)))), "{got:?}");
    assert!(got.contains(&("explorer", "w", &IssueKind::UnknownAction("no.such.action".into()))), "{got:?}");
    assert!(got.contains(&("overlay.commands", "a", &IssueKind::TextKey)), "{got:?}");
    assert!(got.contains(&("editor.vim.insert", "space", &IssueKind::TextKey)), "{got:?}");
    assert!(got.contains(&("editor.vim.normal", "d", &IssueKind::Reserved)), "{got:?}");
    assert!(got.contains(&("explorer", "ctrl+e", &IssueKind::Protected)), "{got:?}");
    assert!(got.contains(&("editor.vim.normal", "ctrl+e", &IssueKind::Protected)), "{got:?}");
    assert!(
        matches!(got.iter().find(|g| g.1 == "g"), Some((_, _, IssueKind::Prefix(o))) if o.starts_with('g')),
        "{got:?}"
    );
    // A valid entry after bad ones still applies (`Enter` of the welcome panel).
    assert_eq!(last(&km, Ctx::Welcome, "enter"), act("app.quit"));
    assert_eq!(issues.len(), 10, "{got:?}");
    // Nothing else changed.
    assert_eq!(last(&km, Ctx::Explorer, "g g"), act("explorer.top"));
    assert_eq!(last(&km, Ctx::VimNormal, "ctrl+e"), act("query.execute_current"));
    assert_eq!(last(&km, Ctx::VimNormal, "d"), Resolved::Forward(k("d")));
    assert_eq!(last(&km, Ctx::Explorer, "d"), Resolved::Action(Action::Explorer(ExplorerAction::Delete)));
}

#[test]
fn keybindings_doc_lists_every_context_and_action() {
    let doc = doc::render();
    for c in Ctx::ALL {
        assert!(doc.contains(&format!("## `{}`", c.name())), "{}", c.name());
    }
    for s in action::REGISTRY {
        assert!(doc.contains(&format!("`{}`", s.id)), "{}", s.id);
    }
    assert!(!doc.contains("standard"), "vim keys only");
    assert!(doc.contains("| `Ctrl+E` | `query.execute_current` | Run statement under cursor |  |"));
    assert!(doc.contains("| `j` | `explorer.down` | Explorer: next item | ✓ |"));
    let hangul = |c: &char| ('\u{AC00}'..='\u{D7A3}').contains(c) || ('\u{3130}'..='\u{318F}').contains(c);
    assert!(!doc.chars().any(|c| hangul(&c)), "English only");
    assert!(!doc.contains('\r'));
}

// ── key guide: leader groups, hints, help sections ───────────────────────────

#[test]
fn leader_groups_list_only_what_leads_to_an_action() {
    let km = Keymap::default();
    let shown = |ctx, prefix: &str| -> Vec<(String, Child)> {
        km.children(ctx, &k(prefix)).into_iter().map(|(c, child)| (c.label(), child)).collect()
    };
    let root = shown(Ctx::Explorer, "space");
    let keys: Vec<&str> = root.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [",", "/", "1", "2", "3", "4", "5", "6", "7", "8", "9", "?", "c", "r", "s", "t"],
        "the monitor has no action yet"
    );
    assert!(root.contains(&("r".into(), Child::Group(Some(Label::GroupResult)))));
    assert!(root.contains(&("s".into(), Child::Group(Some(Label::GroupScript)))));
    assert!(root.contains(&("t".into(), Child::Group(Some(Label::GroupTab)))));
    assert!(root.contains(&(",".into(), Child::Action(action::by_id("settings.open").unwrap().action))));
    assert!(root.contains(&("c".into(), Child::Group(Some(Label::GroupConn)))));
    assert!(root.contains(&("/".into(), Child::Action(action::by_id("commands.open").unwrap().action))));
    assert_eq!(km.group_label(Ctx::Explorer, &k("space r")), Some(Label::GroupResult));
    let results: Vec<String> = shown(Ctx::VimNormal, "space r").into_iter().map(|(k, _)| k).collect();
    assert_eq!(results, ["#", "+", "-", "I", "[", "]", "a", "h", "i", "n", "p", "y", "z"]);
    assert_eq!(km.group_label(Ctx::Explorer, &k("space r y")), Some(Label::GroupCopySelection));
    let formats: String = shown(Ctx::Grid, "space r a").into_iter().map(|(k, _)| k).collect();
    assert_eq!(formats, "JTchijlmntux", "every format, sorted by key");
    // No leader while typing; an empty group resolves to nothing.
    assert!(shown(Ctx::VimInsert, "space").is_empty());
    assert_eq!(last(&km, Ctx::Explorer, "space m"), Resolved::Unbound(k("space m")));
    assert_eq!(km.resolve_seq(Ctx::Explorer, &k("space c")), (None, true));
    // A user action on a group's keys clashes with the group.
    let (km2, issues) = user(&[("nav", "space c", "explorer.top")]);
    assert!(matches!(issues[..], [Issue { kind: IssueKind::Prefix(_), .. }]), "{issues:?}");
    assert_eq!(km2, km);
}

#[test]
fn hint_keys_prefer_the_innermost_usable_key() {
    let km = Keymap::default();
    let hint = |id: &str, ctx, enhanced| {
        km.hint_keys(action::by_id(id).unwrap().action, ctx, enhanced).map(|k| keys::label(&k))
    };
    assert_eq!(hint("query.execute_current", Ctx::VimNormal, false).as_deref(), Some("Ctrl+E"));
    assert_eq!(hint("query.execute_current", Ctx::VimNormal, true).as_deref(), Some("Ctrl+Enter"));
    // A single key before a sequence: the help shows as F1 everywhere.
    for ctx in [Ctx::Explorer, Ctx::Grid, Ctx::VimNormal, Ctx::VimVisual, Ctx::VimInsert] {
        assert_eq!(hint("help.context", ctx, false).as_deref(), Some("F1"), "{ctx:?}");
    }
    assert_eq!(hint("commands.open", Ctx::Explorer, false).as_deref(), Some(":"));
    assert_eq!(hint("help.all", Ctx::Explorer, false), None);
    // Every hint names a real action with a key in its context.
    for (ctx, list) in HINTS {
        for (id, _) in *list {
            assert!(hint(id, *ctx, false).is_some(), "{} {id}", ctx.name());
        }
    }
}

#[test]
fn help_sections_show_keys_the_editor_takes() {
    let km = Keymap::default();
    let close = action::by_id("tab.close").unwrap().action;
    let ws = km.section(Ctx::Workspace, Some(Ctx::VimInsert));
    let e = ws.iter().find(|e| e.action == close).unwrap();
    assert_eq!((e.keys.len(), e.editor_keys.clone()), (0, vec![k("ctrl+w")]), "vim Insert deletes a word");
    // From vim Normal nothing of the workspace is hidden; without a context every key shows.
    let prev = action::by_id("pane.prev").unwrap().action;
    let e = km.section(Ctx::Workspace, Some(Ctx::VimNormal)).into_iter().find(|e| e.action == prev).unwrap();
    assert_eq!((e.keys.len(), e.editor_keys.len()), (2, 0));
    assert!(km.section(Ctx::VimInsert, None).iter().any(|e| e.keys.contains(&k("ctrl+n"))));
    assert!(km.section(Ctx::CellViewer, None).is_empty(), "its keys are the widget's");
}

#[test]
fn help_opens_with_space_question_and_f1_everywhere() {
    let km = Keymap::default();
    let help = Target::Action(action::by_id("help.context").unwrap().action);
    // The only keys: `Space ?` for the panes that are not text input, F1 for the workspace.
    let mut keys: Vec<(Ctx, String)> =
        km.bindings().iter().filter(|b| b.target == help).map(|b| (b.ctx, keys::label(&b.keys))).collect();
    keys.sort();
    assert_eq!(keys, [(Ctx::Workspace, "F1".to_string()), (Ctx::Nav, "Space ?".to_string())]);
    for ctx in Ctx::ALL.into_iter().filter(|c| c.chain().contains(&Ctx::Workspace)) {
        assert_eq!(last(&km, ctx, "f1"), act("help.context"), "{ctx:?}");
        if ctx.chain().contains(&Ctx::Nav) {
            assert_eq!(last(&km, ctx, "space ?"), act("help.context"), "{ctx:?}");
        }
        // `?` alone opens nothing: vim's backward search in the editor, free elsewhere.
        assert_ne!(last(&km, ctx, "?"), act("help.context"), "{ctx:?}");
    }
}
