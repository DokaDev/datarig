//! The `:` command line: its list of candidates, its keys and running what was
//! chosen or typed. The grammar of the commands is in [`super::command`].

use super::command::{self, ArgCompletion, ArgKind, COMMANDS, CommandSpec, Parsed, SETTINGS, SetError, Setting};
use super::*;
use crate::widgets::text_input::InputResult;

/// A name `:use` looked for in a list read earlier and did not find there.
#[derive(Clone, Debug)]
enum Missing {
    Database(String),
    /// A schema, of this database.
    Schema(String, String),
}

impl Missing {
    /// What to read again.
    fn ask(&self) -> UseAsk {
        match self {
            Missing::Database(_) => UseAsk::Databases,
            Missing::Schema(db, _) => UseAsk::Schemas(db.clone()),
        }
    }

    fn name(&self) -> String {
        match self {
            Missing::Database(n) | Missing::Schema(_, n) => n.clone(),
        }
    }

    /// Why nothing switches, once the server's answer does not have it either.
    fn refusal(&self) -> Msg {
        match self.clone() {
            Missing::Database(database) => Msg::ContextUnknownDatabase { database },
            Missing::Schema(database, schema) => Msg::ContextUnknownSchema { schema, database },
        }
    }
}

/// What a pending `:use` asked the server for: the databases, or the schemas of a database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UseAsk {
    Databases,
    Schemas(String),
}

/// A `:use` waiting for the server, bound to its tab and binding and to the
/// profile's connection (generation).
#[derive(Clone, Debug)]
pub(crate) struct PendingUse {
    tab: TabId,
    binding: u64,
    profile: ProfileId,
    generation: u64,
    database: Option<String>,
    schema: Option<String>,
    /// What was asked, in order (each at most once).
    asked: Vec<UseAsk>,
}

/// One row of the command line's list, ready to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRow {
    /// What the row completes to (`:conn <profile>`, a profile, `language=ko`); empty for an
    /// action.
    pub name: Localized,
    pub label: Localized,
    /// Its keys in the context below the command line, if any.
    pub keys: String,
}

/// The action a setting value stands for, if there is one (its keys are shown next to it).
fn setting_action(s: Setting) -> Option<Action> {
    match s {
        Setting::Language(l) => Some(Action::SetLanguage(l)),
        Setting::Editor(m) => Some(Action::SetEditorMode(m)),
        Setting::Icons(i) => Some(Action::SetIcons(i)),
        Setting::DefaultSource(d) => Some(Action::SetDefaultSource(d)),
        Setting::CommandsPosition(_)
        | Setting::DetailView(_)
        | Setting::Clipboard(_)
        | Setting::CopyHeader(_)
        | Setting::CursorShape(_) => None,
    }
}

impl App {
    /// `Ctrl+K` / `:`: open the command line, or close it when it is open.
    pub(super) fn toggle_commands(&mut self) {
        self.overlays.close(OverlayKind::WhichKey);
        if self.overlays.is_open(OverlayKind::Commands) {
            self.overlays.close(OverlayKind::Commands);
        } else if !self.layout.too_small {
            let t = self.tab_mut();
            t.popup = None;
            t.completion_due = None;
            self.overlays.push(Overlay::Commands(CommandLine {
                input: TextInput::default(),
                items: Vec::new(),
                selected: 0,
                error: None,
                picked: false,
            }));
            self.refresh_commands();
        }
    }

    /// A command can run here: the action it runs, if any, is available now.
    /// What `:use` completes: the active tab's server's databases, then `.schema` for the
    /// schemas of the tab's database, as far as they are known.
    fn context_names(&self) -> Vec<String> {
        let quote = |s: &str| {
            let plain = !s.is_empty()
                && s.chars().all(|c| c == '_' || c.is_ascii_lowercase() || c.is_ascii_digit())
                && !s.starts_with(|c: char| c.is_ascii_digit());
            if plain { s.to_string() } else { format!("\"{}\"", s.replace('"', "\"\"")) }
        };
        let t = self.tab();
        let Some(p) = t.profile else { return Vec::new() };
        let mut out: Vec<String> = match self.conns.get(p).and_then(|c| c.databases.as_ref()) {
            Some(Ok(dbs)) => dbs.iter().map(|d| quote(d)).collect(),
            _ => Vec::new(),
        };
        let schemas: Vec<String> = match self.other_database(t) {
            None => {
                self.conns.get(p).map(|c| c.tree.schemas.iter().map(|s| s.name.clone()).collect()).unwrap_or_default()
            }
            Some(db) => self.conns.aux(p, db).and_then(|a| a.schemas.clone()).and_then(Result::ok).unwrap_or_default(),
        };
        out.extend(schemas.iter().map(|s| format!(".{}", quote(s))));
        out
    }

    /// Whether database `db` (`None`: the profile's own) and `schema` of profile `p` may be
    /// there: missing only when a list that was read does not have the name (that
    /// list may be out of date, so the server is asked again before anything is refused).
    fn context_known(&self, p: ProfileId, db: Option<&str>, schema: Option<&str>) -> Result<(), Missing> {
        let own = self.own_database(p);
        let database = db.unwrap_or(&own).to_string();
        if let Some(Ok(dbs)) = self.conns.get(p).and_then(|c| c.databases.as_ref())
            && !dbs.contains(&database)
        {
            return Err(Missing::Database(database));
        }
        let Some(schema) = schema else { return Ok(()) };
        let schemas: Option<Vec<String>> = if database == own {
            self.conns
                .get(p)
                .filter(|c| c.connected && !c.tree.schemas_loading)
                .map(|c| c.tree.schemas.iter().map(|s| s.name.clone()).collect())
        } else {
            self.conns.aux(p, &database).and_then(|a| a.schemas.clone()).and_then(Result::ok)
        };
        match schemas {
            Some(list) if !list.iter().any(|s| s == schema) => Err(Missing::Schema(database, schema.to_string())),
            _ => Ok(()),
        }
    }

    /// `:use` of a name `missing` from a list read earlier: that list is read
    /// again from the server (the databases on the metadata session, the schemas on the metadata
    /// or aux session of that database: one catalog query), and the switch waits for the answer.
    /// It is refused only when the server's answer does not have the name either; when the
    /// server cannot be asked, the status bar says the list may be out of date.
    fn ask_for_use(&mut self, mut pending: PendingUse, missing: Missing) {
        let ask = missing.ask();
        if pending.asked.contains(&ask) {
            return self.flash(Notice::new(missing.refusal(), Level::Error));
        }
        let p = pending.profile;
        let sent = match &ask {
            UseAsk::Databases => self.ask_use_databases(p),
            UseAsk::Schemas(db) => self.ask_use_schemas(p, db),
        };
        let name = missing.name();
        if !sent {
            return self.flash(Notice::new(Msg::ContextListStale { name }, Level::Warning));
        }
        pending.asked.push(ask);
        self.pending_use = Some(pending);
        self.flash(Notice::new(Msg::ContextChecking { name }, Level::Info));
    }

    /// Read profile `p`'s databases again for `:use`; `false` when it cannot be asked.
    fn ask_use_databases(&mut self, p: ProfileId) -> bool {
        let Some(c) = self.conns.get(p).filter(|c| c.connected && c.meta.is_some()) else { return false };
        // One on its way answers too.
        if !c.databases_asked {
            self.ask_databases(p, true);
        }
        true
    }

    /// Read the schemas of profile `p`'s database `db` again for `:use`; `false` when they
    /// cannot be asked for.
    fn ask_use_schemas(&mut self, p: ProfileId, db: &str) -> bool {
        if db == self.own_database(p) {
            let Some(c) = self.conns.get_mut(p).filter(|c| c.connected && c.meta.is_some()) else { return false };
            c.tree.schemas_loading = true;
            self.send_meta(p, DbCommand::LoadSchemas);
            return true;
        }
        // Another database's aux session: one that opens reads its schemas first.
        let open = self.conns.aux(p, db).is_some_and(|a| a.session.is_some());
        self.ensure_aux(p, db);
        let Some(a) = self.conns.aux_mut(p, db).filter(|a| a.session.is_some()) else { return false };
        if open {
            a.tree.schemas_loading = true;
            if let Some(s) = &a.session {
                s.send(DbCommand::LoadSchemas);
            }
        }
        true
    }

    /// The server answered what a pending `:use` asked for (`ok`: the list was read): the switch
    /// goes on when the answer has the name, is refused when it does not, and the list may be out
    /// of date when it could not be read. Only for the tab, binding and profile connection it was
    /// asked for, while that tab is the active one.
    pub(super) fn use_answered(&mut self, p: ProfileId, answer: UseAsk, ok: bool) {
        let Some(pending) = self.pending_use.take_if(|u| u.profile == p && u.asked.last() == Some(&answer)) else {
            return;
        };
        let same = self.tab().id == pending.tab
            && self.tab().binding == pending.binding
            && self.conns.is_current(p, pending.generation);
        if !same {
            return;
        }
        match self.context_known(p, pending.database.as_deref(), pending.schema.as_deref()) {
            _ if !ok => {
                let name = match answer {
                    UseAsk::Databases => pending.database.clone().unwrap_or_else(|| self.own_database(p)),
                    UseAsk::Schemas(_) => pending.schema.clone().unwrap_or_default(),
                };
                self.flash(Notice::new(Msg::ContextListStale { name }, Level::Warning));
            }
            Ok(()) => {
                // "Asking the server again" is over.
                if self.transient.as_ref().is_some_and(|(n, _)| matches!(n.msg, Msg::ContextChecking { .. })) {
                    self.transient = None;
                }
                let ctx = self.session_context(p, pending.database.clone(), pending.schema.clone());
                self.request_set_context(Some(ctx));
            }
            Err(missing) => self.ask_for_use(pending, missing),
        }
    }

    fn command_available(&self, spec: &CommandSpec) -> bool {
        spec.action.is_none_or(|a| (action::spec(a).when)(self))
    }

    /// The list for `input`: argument completions while an argument is typed, otherwise the
    /// commands the first word starts (exact names first) and the actions matching the text.
    /// Only what can run now is listed; an action a listed command already runs is left out.
    /// The exact name of a command that cannot run here lists nothing.
    pub fn command_items(&self, input: &str) -> Vec<CommandItem> {
        let parsed = command::parse(input);
        if let Parsed::Command { spec, arg, arg_started: true } = parsed
            && let Some(kind) = spec.arg
        {
            let c = COMMANDS.iter().position(|s| s.command == spec.command).unwrap_or(0);
            let scripts = self.script_names();
            let contexts = if kind == ArgKind::Context { self.context_names() } else { Vec::new() };
            let names: Vec<&str> = match kind {
                ArgKind::Script => scripts.iter().map(String::as_str).collect(),
                ArgKind::Context => contexts.iter().map(String::as_str).collect(),
                _ => self.profiles.iter().map(|p| p.name.as_str()).collect(),
            };
            return command::complete_arg(kind, arg, &names)
                .into_iter()
                .map(|arg| CommandItem::Arg { command: c, arg })
                .collect();
        }
        let text = input.trim();
        // A command typed by its name that cannot run here lists nothing, so `Enter` says it is
        // not available (never runs an action whose name merely matches the letters).
        if COMMANDS.iter().any(|c| c.is_named(text) && !self.command_available(c)) {
            return Vec::new();
        }
        let mut out: Vec<CommandItem> = Vec::new();
        if !text.contains(char::is_whitespace) {
            let mut cmds: Vec<usize> = (0..COMMANDS.len())
                .filter(|&i| COMMANDS[i].starts_with(text) && self.command_available(&COMMANDS[i]))
                .collect();
            cmds.sort_by_key(|&i| !COMMANDS[i].is_named(text));
            out.extend(cmds.into_iter().map(CommandItem::Command));
        }
        let covered: Vec<Action> = out
            .iter()
            .filter_map(|c| if let CommandItem::Command(i) = c { COMMANDS[*i].action } else { None })
            .collect();
        out.extend(
            action::search(text, &self.i18n)
                .into_iter()
                .filter(|&i| (REGISTRY[i].when)(self) && !covered.contains(&REGISTRY[i].action))
                .map(CommandItem::Action),
        );
        out
    }

    /// Recompute the list after the input changed (the selection goes back to the top).
    pub(super) fn refresh_commands(&mut self) {
        let Some(c) = self.overlays.command_line() else { return };
        let items = self.command_items(c.input.text());
        if let Some(c) = self.overlays.command_line_mut() {
            c.items = items;
            c.selected = 0;
            c.error = None;
            c.picked = false;
        }
    }

    /// Replace the input (a completion that needs more typing) and refresh the list.
    fn set_command_input(&mut self, text: &str) {
        if let Some(c) = self.overlays.command_line_mut() {
            c.input.set(text);
        }
        self.refresh_commands();
    }

    /// A `:use` argument is typed and no completion was picked yet: `Enter` runs the text as
    /// it is and no entry is shown selected. The command, and the argument.
    pub fn typed_context(&self) -> Option<(&'static CommandSpec, String)> {
        let c = self.overlays.command_line()?;
        match command::parse(c.input.text()) {
            Parsed::Command { spec, arg, arg_started: true }
                if spec.arg == Some(ArgKind::Context) && !arg.trim().is_empty() && !c.picked =>
            {
                Some((spec, arg.to_string()))
            }
            _ => None,
        }
    }

    /// Keys of the command line (`overlay.commands`, text input).
    pub(super) fn command_key(&mut self, key: KeyEvent, repeat: bool) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // The first `Tab`/`↑`/`↓` on a typed `:use` argument picks the top completion.
        let first_pick = self.typed_context().is_some();
        let Some(c) = self.overlays.command_line_mut() else { return };
        let n = c.items.len().max(1);
        let moves = matches!(key.code, KeyCode::Up | KeyCode::BackTab | KeyCode::Down | KeyCode::Tab)
            || ctrl && matches!(key.code, KeyCode::Char('p' | 'n'));
        if moves {
            c.picked = true;
            if first_pick {
                return;
            }
        }
        match key.code {
            KeyCode::Esc => self.overlays.close(OverlayKind::Commands),
            KeyCode::Enter if !repeat => self.command_enter(),
            KeyCode::Enter => {}
            KeyCode::Backspace if c.input.text().is_empty() => self.overlays.close(OverlayKind::Commands),
            KeyCode::Up | KeyCode::BackTab => c.selected = (c.selected + n - 1) % n,
            KeyCode::Char('p') if ctrl => c.selected = (c.selected + n - 1) % n,
            KeyCode::Down | KeyCode::Tab => c.selected = (c.selected + 1) % n,
            KeyCode::Char('n') if ctrl => c.selected = (c.selected + 1) % n,
            _ => {
                if c.input.handle_key(&key) == InputResult::Changed {
                    self.refresh_commands();
                }
            }
        }
    }

    /// `Enter`: run the selected entry. A command that still needs its argument, or a setting
    /// without its value, is completed into the input instead. With nothing listed, the typed
    /// text runs as a command; what cannot run shows an error and the command line stays.
    fn command_enter(&mut self) {
        // A typed `:use` argument runs as it is (a completion only once picked).
        if let Some((spec, arg)) = self.typed_context() {
            if let Err(e) = self.run_command(spec, &arg)
                && let Some(c) = self.overlays.command_line_mut()
            {
                c.error = Some(e);
            }
            return;
        }
        let Some(c) = self.overlays.command_line() else { return };
        let input = c.input.text().to_string();
        let result = match c.items.get(c.selected).copied() {
            Some(CommandItem::Action(i)) => {
                self.overlays.close(OverlayKind::Commands);
                self.dispatch(REGISTRY[i].action);
                Ok(())
            }
            Some(CommandItem::Command(i)) if COMMANDS[i].arg.is_some_and(|k| !k.optional()) => {
                self.set_command_input(&format!("{} ", COMMANDS[i].name));
                Ok(())
            }
            Some(CommandItem::Command(i)) => self.run_command(&COMMANDS[i], ""),
            Some(CommandItem::Arg { command, arg: ArgCompletion::Profile(p) }) => {
                let name = self.profiles.get(p).map(|p| p.name.clone()).unwrap_or_default();
                self.run_command(&COMMANDS[command], &name)
            }
            Some(CommandItem::Arg { command, arg: ArgCompletion::Context(i) }) => {
                let name = self.context_names().get(i).cloned().unwrap_or_default();
                self.run_command(&COMMANDS[command], &name)
            }
            Some(CommandItem::Arg { command, arg: ArgCompletion::Script(i) }) => {
                let name = self.script_names().get(i).cloned().unwrap_or_default();
                self.run_command(&COMMANDS[command], &name)
            }
            Some(CommandItem::Arg { command, arg: ArgCompletion::SetKey(k) }) => {
                self.set_command_input(&format!("{} {}=", COMMANDS[command].name, SETTINGS[k].key));
                Ok(())
            }
            Some(CommandItem::Arg { arg: ArgCompletion::SetValue(k, v), .. }) => {
                self.overlays.close(OverlayKind::Commands);
                self.apply_setting(SETTINGS[k].values[v].1);
                Ok(())
            }
            Some(CommandItem::Arg { command, arg: ArgCompletion::Format(i) }) => {
                self.run_command(&COMMANDS[command], super::copy::CopyFormat::MENU[i].name())
            }
            Some(CommandItem::Arg { command, arg: ArgCompletion::Scope(f, sc) }) => {
                let arg =
                    format!("{} {}", super::copy::CopyFormat::MENU[f].name(), super::copy::CopyScope::ALL[sc].name());
                self.run_command(&COMMANDS[command], &arg)
            }
            None => match command::parse(&input) {
                Parsed::Empty => {
                    self.overlays.close(OverlayKind::Commands);
                    Ok(())
                }
                // A command given an argument it does not take (and no action matched the text).
                Parsed::Text { word } if command::by_name(word).is_some() => {
                    Err(Notice::new(Msg::CommandsErrorNoArgs { name: word.to_string() }, Level::Error))
                }
                Parsed::Text { word } => {
                    Err(Notice::new(Msg::CommandsErrorUnknown { name: word.to_string() }, Level::Error))
                }
                Parsed::Command { spec, arg, .. } => self.run_command(spec, arg),
            },
        };
        if let Err(e) = result
            && let Some(c) = self.overlays.command_line_mut()
        {
            c.error = Some(e);
        }
    }

    /// Run command `spec` with argument `arg`, closing the command line first. An argument
    /// that does not fit, or a command that cannot run here, is an error (and nothing runs).
    fn run_command(&mut self, spec: &'static CommandSpec, arg: &str) -> Result<(), Notice> {
        let name = spec.name.to_string();
        let err = |m: Msg| Err(Notice::new(m, Level::Error));
        match (spec.command, spec.action) {
            (command::Command::Conn, _) => {
                if arg.is_empty() {
                    return err(Msg::Label(Label::CommandsErrorConnUsage));
                }
                let exact = self.profiles.iter().position(|p| p.name == arg);
                let folded = || {
                    let hits: Vec<usize> = (0..self.profiles.len())
                        .filter(|&i| self.profiles[i].name.to_lowercase() == arg.to_lowercase())
                        .collect();
                    (hits.len() == 1).then(|| hits[0])
                };
                let Some(i) = exact.or_else(folded) else {
                    return err(Msg::ProfileUnknown { name: arg.to_string() });
                };
                self.overlays.close(OverlayKind::Commands);
                let id = self.profiles[i].id;
                self.quick_choose(id, QuickPurpose::Open, None);
            }
            (command::Command::Set, _) => {
                let setting = match command::parse_set(arg) {
                    Ok(s) => s,
                    Err(SetError::Usage) => return err(Msg::Label(Label::CommandsErrorSetUsage)),
                    Err(SetError::UnknownKey(key)) => {
                        return err(Msg::CommandsErrorSetKey { key, keys: command::setting_keys() });
                    }
                    Err(SetError::BadValue { key, value, values }) => {
                        return err(Msg::CommandsErrorSetValue { key: key.to_string(), value, values });
                    }
                };
                self.overlays.close(OverlayKind::Commands);
                self.apply_setting(setting);
            }
            (command::Command::Recover, _) => {
                if !arg.is_empty() {
                    return err(Msg::CommandsErrorNoArgs { name });
                }
                self.overlays.close(OverlayKind::Commands);
                self.open_recover();
            }
            (command::Command::Quit, _) => {
                if !arg.is_empty() {
                    return err(Msg::CommandsErrorNoArgs { name });
                }
                self.overlays.close(OverlayKind::Commands);
                self.close_or_quit();
            }
            (command::Command::Write | command::Command::WriteQuit, _) => {
                if !self.scripts_available() {
                    return err(Msg::Label(Label::ScriptsNoDataDir));
                }
                let close = spec.command == command::Command::WriteQuit;
                let id = self.tab().id;
                if !arg.is_empty() {
                    let before = self.tab().doc.script.clone();
                    self.save_as(id, arg).map_err(|l| Notice::new(l, Level::Error))?;
                    self.overlays.close(OverlayKind::Commands);
                    // Only once it was written (a failed write said why and keeps the tab).
                    let saved =
                        self.tab().id == id && self.tab().doc.script.is_some() && self.tab().doc.script != before;
                    if close && saved {
                        self.close_or_quit();
                    }
                    return Ok(());
                }
                self.overlays.close(OverlayKind::Commands);
                if self.tab().script().is_none() {
                    self.ask_script_name(id, close);
                } else {
                    self.save_current();
                    if close && !self.tab().doc.conflict {
                        match self.tab().doc.save_error.clone() {
                            None => self.close_or_quit(),
                            // Said every time, not only on the first failure: the tab stays.
                            Some(reason) => {
                                let error = self.reason_text(&reason);
                                self.flash(Notice::new(Msg::ScriptsWriteQuitFailed { error }, Level::Error));
                            }
                        }
                    }
                }
            }
            (command::Command::Copy, _) => {
                use super::copy::{CopyFormat, CopyScope};
                if arg.is_empty() {
                    return err(Msg::Label(Label::CommandsErrorCopyUsage));
                }
                let (word, rest) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
                let rest = rest.trim();
                let scope = match rest.to_lowercase().as_str() {
                    "" => None,
                    s => CopyScope::ALL.into_iter().find(|c| c.name() == s),
                };
                // `:copy insert <schema.table>`: SQL INSERT statements for the table named (a
                // word that is not a scope; a table named like one is quoted: "all").
                if word.eq_ignore_ascii_case("insert") && !rest.is_empty() && scope.is_none() {
                    self.overlays.close(OverlayKind::Commands);
                    self.copy_into(rest);
                    return Ok(());
                }
                let Some(format) = CopyFormat::parse(word) else {
                    return err(Msg::CommandsErrorCopyFormat { format: word.to_string() });
                };
                if !rest.is_empty() && scope.is_none() {
                    return err(Msg::Label(Label::CommandsErrorCopyUsage));
                }
                self.overlays.close(OverlayKind::Commands);
                match scope {
                    Some(scope) => self.copy_scoped(scope, format),
                    None => self.copy(super::copy::CopyWhat::All, format, false),
                }
            }
            (command::Command::Use, _) => {
                if !self.command_available(spec) {
                    return err(Msg::CommandsErrorUnavailable { name });
                }
                if arg.is_empty() {
                    self.overlays.close(OverlayKind::Commands);
                    self.request_set_context(None);
                    return Ok(());
                }
                let Some((db, schema)) = command::parse_context(arg) else {
                    return err(Msg::Label(Label::CommandsErrorUseUsage));
                };
                let Some(p) = self.tab().profile else {
                    self.overlays.close(OverlayKind::Commands);
                    return Ok(());
                };
                let db = db.or_else(|| self.tab().context.database.clone());
                // A name the app does not know yet goes to the server, which says;
                // one a list read earlier does not have is asked for again first.
                self.overlays.close(OverlayKind::Commands);
                if let Err(missing) = self.context_known(p, db.as_deref(), schema.as_deref()) {
                    let t = self.tab();
                    let generation = self.conns.get(p).map_or(0, |c| c.generation);
                    let pending = PendingUse {
                        tab: t.id,
                        binding: t.binding,
                        profile: p,
                        generation,
                        database: db,
                        schema,
                        asked: Vec::new(),
                    };
                    self.ask_for_use(pending, missing);
                    return Ok(());
                }
                let ctx = self.session_context(p, db, schema);
                self.request_set_context(Some(ctx));
            }
            (command::Command::Edit, _) => {
                // Without a name: the tree of the saved queries.
                if arg.is_empty() {
                    self.overlays.close(OverlayKind::Commands);
                    self.open_script_chooser();
                    return Ok(());
                }
                self.open_script_named(arg)?;
                self.overlays.close(OverlayKind::Commands);
            }
            (_, Some(a)) => {
                if !arg.is_empty() {
                    return err(Msg::CommandsErrorNoArgs { name });
                }
                if !(action::spec(a).when)(self) {
                    return err(Msg::CommandsErrorUnavailable { name });
                }
                self.overlays.close(OverlayKind::Commands);
                self.dispatch(a);
            }
            (_, None) => {}
        }
        Ok(())
    }

    /// The saved queries as `:e` takes them: paths without the extension.
    fn script_names(&self) -> Vec<String> {
        self.script_list
            .iter()
            .filter(|e| !e.folder)
            .map(|e| datarig_core::scripts::stem_path(&e.path).to_string())
            .collect()
    }

    pub(super) fn apply_setting(&mut self, s: Setting) {
        match s {
            Setting::Language(l) => self.set_language(l),
            Setting::Editor(m) => self.set_editor_mode(m),
            Setting::Icons(i) => self.set_icons(i),
            Setting::DefaultSource(d) => self.set_default_source(d),
            Setting::CommandsPosition(p) => self.set_prefs(s, |x| x.commands_position = p),
            Setting::DetailView(v) => self.set_prefs(s, |x| x.detail_view = v),
            Setting::Clipboard(c) => self.set_prefs(s, |x| x.clipboard = c),
            Setting::CopyHeader(c) => self.set_prefs(s, |x| x.copy_header = c),
            Setting::CursorShape(c) => self.set_prefs(s, |x| x.cursor_shape = c),
        }
    }

    /// Change one of the display, result or clipboard settings (`s`), save it and say so: its name and new value.
    fn set_prefs(&mut self, s: Setting, change: impl FnOnce(&mut datarig_core::config::Prefs)) {
        change(&mut self.prefs);
        let saved = self.persist();
        let spec = SETTINGS.iter().find(|x| x.values.iter().any(|v| v.1 == s));
        let msg = spec.and_then(|spec| {
            let value = spec.values.iter().find(|v| v.1 == s)?;
            Some(Msg::SettingChanged {
                name: self.i18n.label(spec.label).to_string(),
                value: self.i18n.label(value.2).to_string(),
            })
        });
        if let Some(m) = saved.or(msg.map(|m| Notice::new(m, Level::Info))) {
            self.flash(m);
        }
    }

    /// A note about what is being typed, shown above the input line: the `icons` setting's
    /// description with a preview of the glyphs, while `:set icons` is being typed.
    pub fn command_note(&self) -> Option<Localized> {
        let c = self.overlays.command_line()?;
        let Parsed::Command { spec, arg, arg_started: true } = command::parse(c.input.text()) else { return None };
        let key = arg.split('=').next().unwrap_or("").trim();
        (spec.command == command::Command::Set && !key.is_empty() && "icons".starts_with(key))
            .then(|| self.i18n.msg(&Msg::SettingIconsHelp { preview: crate::icons::preview() }))
    }

    /// The rows of the command line's list, in its order.
    pub fn command_rows(&self) -> Vec<CommandRow> {
        let Some(c) = self.overlays.command_line() else { return Vec::new() };
        let ctx = self.context_below_commands();
        let keys = |a: Action| self.keymap.keys_label_in(a, ctx, self.enhanced_keys);
        c.items
            .iter()
            .map(|item| match *item {
                CommandItem::Action(i) => CommandRow {
                    name: Localized::verbatim(""),
                    label: self.i18n.label(REGISTRY[i].label),
                    keys: keys(REGISTRY[i].action),
                },
                CommandItem::Command(i) => {
                    let spec = &COMMANDS[i];
                    let mut name = format!(":{}", spec.name);
                    for a in spec.aliases {
                        name.push_str(&format!(", :{a}"));
                    }
                    if let Some(kind) = spec.arg {
                        let (arg, optional) = match kind {
                            ArgKind::Profile => (Label::CommandsArgProfile, false),
                            ArgKind::Setting => (Label::CommandsArgSetting, false),
                            ArgKind::Script => (Label::CommandsArgScript, true),
                            ArgKind::NewScript => (Label::CommandsArgScript, true),
                            ArgKind::Format => (Label::CommandsArgFormat, false),
                            ArgKind::Context => (Label::CommandsArgContext, true),
                        };
                        let arg = arg.text(self.i18n.lang);
                        name.push_str(&if optional { format!(" [{arg}]") } else { format!(" {arg}") });
                    }
                    CommandRow {
                        // The command's syntax around its localized argument name.
                        name: Localized::verbatim(name),
                        label: self.i18n.label(spec.label),
                        keys: spec.action.map(keys).unwrap_or_default(),
                    }
                }
                CommandItem::Arg { arg: ArgCompletion::Profile(p), .. } => {
                    let p = &self.profiles[p];
                    let (addr, db, _) = p.endpoint();
                    CommandRow {
                        name: Localized::verbatim(p.name.clone()),
                        label: Localized::verbatim(format!("{addr}/{db}")),
                        keys: String::new(),
                    }
                }
                CommandItem::Arg { arg: ArgCompletion::Context(i), .. } => CommandRow {
                    name: Localized::verbatim(self.context_names().get(i).cloned().unwrap_or_default()),
                    label: Localized::verbatim(""),
                    keys: String::new(),
                },
                CommandItem::Arg { arg: ArgCompletion::Script(i), .. } => CommandRow {
                    name: Localized::verbatim(self.script_names().get(i).cloned().unwrap_or_default()),
                    label: Localized::verbatim(""),
                    keys: String::new(),
                },
                CommandItem::Arg { arg: ArgCompletion::Format(i), .. } => {
                    let f = super::copy::CopyFormat::MENU[i];
                    CommandRow {
                        name: Localized::verbatim(f.name()),
                        label: self.i18n.label(f.menu_label()),
                        keys: keys(Action::Copy(super::copy::CopyScope::Selection, f)),
                    }
                }
                CommandItem::Arg { arg: ArgCompletion::Scope(f, sc), .. } => {
                    let (f, sc) = (super::copy::CopyFormat::MENU[f], super::copy::CopyScope::ALL[sc]);
                    let a = Action::Copy(sc, f);
                    CommandRow {
                        name: Localized::verbatim(format!("{} {}", f.name(), sc.name())),
                        label: self.i18n.label(action::spec(a).label),
                        keys: keys(a),
                    }
                }
                CommandItem::Arg { arg: ArgCompletion::SetKey(k), .. } => CommandRow {
                    name: Localized::verbatim(format!("{}=", SETTINGS[k].key)),
                    label: self.i18n.label(SETTINGS[k].label),
                    keys: SETTINGS[k].value_list(),
                },
                CommandItem::Arg { arg: ArgCompletion::SetValue(k, v), .. } => {
                    let (value, setting, label) = SETTINGS[k].values[v];
                    CommandRow {
                        name: Localized::verbatim(format!("{}={value}", SETTINGS[k].key)),
                        label: self.i18n.label(label),
                        keys: setting_action(setting).map(keys).unwrap_or_default(),
                    }
                }
            })
            .collect()
    }
}
