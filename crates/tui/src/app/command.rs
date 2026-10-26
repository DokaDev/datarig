//! Ex-style commands of the `:` command line: a small
//! grammar of named commands with aliases and one typed argument, plus the completions of that
//! argument. Everything here is pure; `App` runs the result (`app/cmdline.rs`).
//!
//! Text that is not a command is a search over the action names (the [`action::search`] of
//! the command line). A command whose grammar does not fit what was typed — a command that
//! takes no argument followed by more words, like `run statement` — is plain text too, so
//! action names that start with a command name still find their action.
//!
//! Adding a command: a [`Command`] variant, its [`CommandSpec`] row in [`COMMANDS`] (name,
//! aliases, [`ArgKind`], label, and the action whose keys the list shows), and its case in
//! `App::run_command`. A new argument type is an [`ArgKind`] variant with its completion in
//! [`complete_arg`].

use super::action::{Action, LangSetting};
use datarig_core::config::{
    AutoPairs, ClipboardSetting, CommandsPosition, CopyHeader, CursorShape, DetailView, EditorClipboard, FormatIndent,
    IconsSetting, KeychainAccess, KeywordCase, RunHints, SizeHeat, SoftTabBackspace, YankFlash,
};
use datarig_core::i18n::Label;
use datarig_core::secret::{DefaultSource, SourceKind};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// `:conn <profile>`: go to a profile's tab, or connect and open a console (like quick
    /// connect).
    Conn,
    /// `:set <setting>=<value>`: change a setting and save it.
    Set,
    Help,
    Run,
    Cancel,
    /// `:q`: close the active tab (asks like `Ctrl+W`); on the last tab, quit.
    Quit,
    /// `:qa`: quit.
    QuitAll,
    /// `:w [name]`: save the tab (a console asks for a name); with a name, save it as that.
    Write,
    /// `:wq [name]`: save, then close the tab.
    WriteQuit,
    /// `:e <name>`: open a saved query.
    Edit,
    /// `:tabnew`: a new console tab.
    TabNew,
    /// `:tabclose`: close the active tab.
    TabClose,
    /// `:tabs`, `:ls`, `:buffers`: the tab list.
    Tabs,
    /// `:tabmove [N|+N|-N|$]` (`:tabm`): move the active tab, as Vim's `:tabmove`.
    TabMove,
    /// `:recover`: pick a closed console from the trash and bring it back.
    Recover,
    /// `:settings`: the settings screen.
    Settings,
    /// `:copy <format> [selection|all]`: copy the selected range (else the cell) or every
    /// fetched row in a format of [`super::copy::CopyFormat::MENU`]; without a scope the
    /// selected range, else every fetched row.
    Copy,
    /// `:use <db>`, `:use <db>.<schema>`, `:use .<schema>`: the active tab's
    /// database and schema; alone, pick them.
    Use,
    /// `:nohlsearch`: no search highlight in the editors until the next search.
    NoHighlight,
    /// `:suspend`: stop until the shell's `fg` (Unix; elsewhere it says it is not supported).
    Suspend,
    /// `:format`: format the statement under the cursor (`:'<,'>format`, with a range, is the
    /// editor's).
    Format,
    /// `:ddl [name]`: the DDL of what the name names on the active tab's connection; alone, of
    /// the explorer's object or the table tab's table.
    Ddl,
    /// `:explain [analyze]`: the plan of the statement under the cursor (with `analyze`, run
    /// and measured).
    Explain,
    /// `:theme copy <from> <name>`, `:theme new <name>`: write a theme file and use it.
    Theme,
    /// `:export [format] [all|fetched|selection] [path]`: export the result to a file.
    Export,
    /// `:commit`, `:rollback` (`:rb`): end the active tab's open transaction.
    Commit,
    Rollback,
    /// `:tx`: what the active tab's open transaction did.
    Tx,
    /// `:session`: what the explorer's (else the active tab's) profile's query sessions start
    /// with, and where each item comes from.
    Session,
}

/// The type of a command's argument (it decides the completions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgKind {
    /// A connection profile name.
    Profile,
    /// `key=value` of [`SETTINGS`].
    Setting,
    /// An existing saved query (`:e`).
    Script,
    /// A new name for a saved query (`:w`); may be left out.
    NewScript,
    /// A format of a copy (`tsv`, `csv`, `json`, …), then a scope (`selection`, `all`).
    Format,
    /// A database and schema of the tab's server (`db`, `db.schema`, `.schema`); may be left
    /// out.
    Context,
    /// An object's name as SQL writes it (`schema.name`, `"Mixed"`, `f(int)`); may be left
    /// out.
    Object,
    /// `analyze`; may be left out.
    Analyze,
    /// `copy <from> <name>` or `new <name>` ([`THEME_ACTIONS`]); `<from>` is a theme name.
    Theme,
    /// Where a tab goes (`:tabmove`): see [`tabmove_target`]; may be left out.
    TabPosition,
    /// An export's format, scope and path (`:export`), each optional.
    Export,
}

impl ArgKind {
    /// The command also runs without it.
    pub fn optional(self) -> bool {
        matches!(
            self,
            ArgKind::NewScript
                | ArgKind::Context
                | ArgKind::Script
                | ArgKind::Object
                | ArgKind::Analyze
                | ArgKind::TabPosition
                | ArgKind::Export
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub command: Command,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// The argument it needs; `None`: it takes none.
    pub arg: Option<ArgKind>,
    pub label: Label,
    /// The action it runs, if it is one: the list shows its keys, and it is offered only where
    /// the action can run.
    pub action: Option<Action>,
}

/// Every command, in list order.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        command: Command::Conn,
        name: "conn",
        aliases: &["connect"],
        arg: Some(ArgKind::Profile),
        label: Label::CommandConn,
        action: None,
    },
    CommandSpec {
        command: Command::Set,
        name: "set",
        aliases: &[],
        arg: Some(ArgKind::Setting),
        label: Label::CommandSet,
        action: None,
    },
    CommandSpec {
        command: Command::Help,
        name: "help",
        aliases: &["h"],
        arg: None,
        label: Label::ActionHelpContext,
        action: Some(Action::Help),
    },
    CommandSpec {
        command: Command::Run,
        name: "run",
        aliases: &[],
        arg: None,
        label: Label::ActionQueryExecuteCurrent,
        action: Some(Action::RunStatement),
    },
    CommandSpec {
        command: Command::Cancel,
        name: "cancel",
        aliases: &[],
        arg: None,
        label: Label::ActionQueryCancel,
        action: Some(Action::CancelQuery),
    },
    CommandSpec {
        command: Command::Quit,
        name: "quit",
        aliases: &["q"],
        arg: None,
        label: Label::CommandQuit,
        action: None,
    },
    CommandSpec {
        command: Command::QuitAll,
        name: "qall",
        aliases: &["qa", "quitall"],
        arg: None,
        label: Label::ActionAppQuit,
        action: Some(Action::Quit),
    },
    CommandSpec {
        command: Command::Write,
        name: "w",
        aliases: &["write"],
        arg: Some(ArgKind::NewScript),
        label: Label::CommandWrite,
        action: Some(Action::ScriptSave),
    },
    CommandSpec {
        command: Command::WriteQuit,
        name: "wq",
        aliases: &["x"],
        arg: Some(ArgKind::NewScript),
        label: Label::CommandWriteQuit,
        action: Some(Action::ScriptSave),
    },
    CommandSpec {
        command: Command::Edit,
        name: "e",
        aliases: &["edit"],
        arg: Some(ArgKind::Script),
        label: Label::CommandEdit,
        action: Some(Action::ScriptOpen),
    },
    CommandSpec {
        command: Command::TabNew,
        name: "tabnew",
        aliases: &[],
        arg: None,
        label: Label::ActionTabNewConsole,
        action: Some(Action::NewTab),
    },
    CommandSpec {
        command: Command::TabClose,
        name: "tabclose",
        aliases: &[],
        arg: None,
        label: Label::ActionTabClose,
        action: Some(Action::CloseTab),
    },
    CommandSpec {
        command: Command::Tabs,
        name: "tabs",
        aliases: &["ls", "buffers"],
        arg: None,
        label: Label::ActionTabList,
        action: Some(Action::TabList),
    },
    CommandSpec {
        command: Command::TabMove,
        name: "tabmove",
        aliases: &["tabm"],
        arg: Some(ArgKind::TabPosition),
        label: Label::CommandTabmove,
        action: None,
    },
    CommandSpec {
        command: Command::Recover,
        name: "recover",
        aliases: &[],
        arg: None,
        label: Label::CommandRecover,
        action: None,
    },
    CommandSpec {
        command: Command::Settings,
        name: "settings",
        aliases: &[],
        arg: None,
        label: Label::CommandSettings,
        action: Some(Action::OpenSettings),
    },
    CommandSpec {
        command: Command::Copy,
        name: "copy",
        aliases: &[],
        arg: Some(ArgKind::Format),
        label: Label::CommandCopy,
        action: Some(Action::Copy(super::copy::CopyScope::Fetched, super::copy::CopyFormat::TsvPlain)),
    },
    CommandSpec {
        command: Command::Use,
        name: "use",
        aliases: &[],
        arg: Some(ArgKind::Context),
        label: Label::ActionTabSetContext,
        action: Some(Action::SetTabContext),
    },
    CommandSpec {
        command: Command::NoHighlight,
        name: "nohlsearch",
        aliases: &["noh", "nohl"],
        arg: None,
        label: Label::CommandNohlsearch,
        action: None,
    },
    CommandSpec {
        command: Command::Suspend,
        name: "suspend",
        aliases: &["sus", "stop"],
        arg: None,
        label: Label::ActionAppSuspend,
        // Listed everywhere; without the action (Windows) it says that it is not supported.
        action: if super::effects::SUSPEND_SUPPORTED { Some(Action::Suspend) } else { None },
    },
    CommandSpec {
        command: Command::Format,
        name: "format",
        aliases: &[],
        arg: None,
        label: Label::ActionEditorFormat,
        action: Some(Action::FormatSql),
    },
    CommandSpec {
        command: Command::Ddl,
        name: "ddl",
        aliases: &[],
        arg: Some(ArgKind::Object),
        label: Label::CommandDdl,
        action: None,
    },
    CommandSpec {
        command: Command::Explain,
        name: "explain",
        aliases: &[],
        arg: Some(ArgKind::Analyze),
        label: Label::CommandExplain,
        action: Some(Action::Explain(false)),
    },
    CommandSpec {
        command: Command::Theme,
        name: "theme",
        aliases: &[],
        arg: Some(ArgKind::Theme),
        label: Label::CommandTheme,
        action: None,
    },
    CommandSpec {
        command: Command::Export,
        name: "export",
        aliases: &[],
        arg: Some(ArgKind::Export),
        label: Label::CommandExport,
        action: Some(Action::Export),
    },
    CommandSpec {
        command: Command::Commit,
        name: "commit",
        aliases: &[],
        arg: None,
        label: Label::ActionTxCommit,
        action: Some(Action::TxCommit),
    },
    CommandSpec {
        command: Command::Rollback,
        name: "rollback",
        aliases: &["rb"],
        arg: None,
        label: Label::ActionTxRollback,
        action: Some(Action::TxRollback),
    },
    CommandSpec {
        command: Command::Tx,
        name: "tx",
        aliases: &[],
        arg: None,
        label: Label::ActionTxSummary,
        action: Some(Action::TxSummary),
    },
    CommandSpec {
        command: Command::Session,
        name: "session",
        aliases: &["startup"],
        arg: None,
        label: Label::ActionSessionShow,
        action: Some(Action::SessionView),
    },
];

impl CommandSpec {
    /// The name or one of the aliases is exactly `word`.
    pub fn is_named(&self, word: &str) -> bool {
        self.name == word || self.aliases.contains(&word)
    }

    /// The name or an alias starts with `word`.
    pub fn starts_with(&self, word: &str) -> bool {
        self.name.starts_with(word) || self.aliases.iter().any(|a| a.starts_with(word))
    }
}

pub fn by_name(word: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|c| c.is_named(word))
}

/// How the command line reads its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parsed<'a> {
    Empty,
    /// A command and its argument text (trimmed). `arg_started`: a space follows the name, so
    /// the argument is being typed (its completions are listed).
    Command {
        spec: &'static CommandSpec,
        arg: &'a str,
        arg_started: bool,
    },
    /// Not a command: a search over the action names. `word` is the first word.
    Text {
        word: &'a str,
    },
}

pub fn parse(input: &str) -> Parsed<'_> {
    let text = input.trim_start();
    if text.is_empty() {
        return Parsed::Empty;
    }
    let (word, rest) = match text.find(char::is_whitespace) {
        Some(i) => (&text[..i], Some(&text[i..])),
        None => (text, None),
    };
    match by_name(word) {
        // A command without an argument followed by more words is plain text (`run statement`).
        Some(spec) if spec.arg.is_none() && rest.is_some_and(|r| !r.trim().is_empty()) => Parsed::Text { word },
        Some(spec) => Parsed::Command { spec, arg: rest.map_or("", str::trim), arg_started: rest.is_some() },
        None => Parsed::Text { word },
    }
}

/// A value of a setting, ready to apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Language(LangSetting),
    Icons(IconsSetting),
    DefaultSource(DefaultSource),
    CommandsPosition(CommandsPosition),
    DetailView(DetailView),
    Clipboard(ClipboardSetting),
    CopyHeader(CopyHeader),
    CursorShape(CursorShape),
    EditorClipboard(EditorClipboard),
    FormatCase(KeywordCase),
    FormatIndent(FormatIndent),
    AutoPairs(AutoPairs),
    SoftTabBackspace(SoftTabBackspace),
    RunHints(RunHints),
    KeychainAccess(KeychainAccess),
    SizeHeat(SizeHeat),
    YankFlash(YankFlash),
    /// `idle_disconnect` (`None`: off).
    IdleDisconnect(Option<Duration>),
    /// `lock_wait` (`None`: off).
    LockWait(Option<Duration>),
}

/// A duration a setting's preset names.
const fn secs(n: u64) -> Option<Duration> {
    Some(Duration::from_secs(n))
}

/// The categories of the settings screen, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SettingGroup {
    Display,
    Editor,
    Results,
    Clipboard,
    Connections,
    Language,
    Secrets,
}

impl SettingGroup {
    pub fn label(self) -> Label {
        match self {
            SettingGroup::Display => Label::SettingsGroupDisplay,
            SettingGroup::Editor => Label::SettingsGroupEditor,
            SettingGroup::Results => Label::SettingsGroupResults,
            SettingGroup::Clipboard => Label::SettingsGroupClipboard,
            SettingGroup::Connections => Label::SettingsGroupConnections,
            SettingGroup::Language => Label::SettingsGroupLanguage,
            SettingGroup::Secrets => Label::SettingsGroupSecrets,
        }
    }
}

/// The settings `:set` changes and the settings screen shows: name, its values (with what each
/// one sets and its label), the setting's label, its description and its category.
pub struct SettingSpec {
    pub key: &'static str,
    pub label: Label,
    pub about: Label,
    pub group: SettingGroup,
    pub values: Values,
}

/// The values of a setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Values {
    /// A fixed list: the name `:set` takes, the [`Setting`] it applies and its label.
    Fixed(&'static [(&'static str, Setting, Label)]),
    /// Theme names: the built-in ones and the user's theme files, listed when asked (the files
    /// change while the app runs), so they are names rather than [`Setting`]s.
    Themes,
}

impl Values {
    /// The fixed values (none for [`Values::Themes`]).
    pub fn fixed(self) -> &'static [(&'static str, Setting, Label)] {
        match self {
            Values::Fixed(v) => v,
            Values::Themes => &[],
        }
    }
}

pub const SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        key: "language",
        label: Label::SettingLanguage,
        about: Label::SettingLanguageAbout,
        group: SettingGroup::Language,
        values: Values::Fixed(&[
            ("en", Setting::Language(LangSetting::En), Label::ActionUiLanguageEn),
            ("ko", Setting::Language(LangSetting::Ko), Label::ActionUiLanguageKo),
            ("auto", Setting::Language(LangSetting::Auto), Label::ActionUiLanguageAuto),
        ]),
    },
    SettingSpec {
        key: "icons",
        label: Label::SettingIcons,
        about: Label::SettingIconsAbout,
        group: SettingGroup::Display,
        values: Values::Fixed(&[
            ("on", Setting::Icons(IconsSetting::On), Label::ActionUiIconsOn),
            ("off", Setting::Icons(IconsSetting::Off), Label::ActionUiIconsOff),
            ("auto", Setting::Icons(IconsSetting::Auto), Label::ActionUiIconsAuto),
        ]),
    },
    SettingSpec {
        key: "secrets.default_source",
        label: Label::SettingDefaultSource,
        about: Label::SettingDefaultSourceAbout,
        group: SettingGroup::Secrets,
        values: Values::Fixed(&[
            ("auto", Setting::DefaultSource(DefaultSource::Auto), Label::ActionSecretsDefaultAuto),
            (
                "keychain",
                Setting::DefaultSource(DefaultSource::Kind(SourceKind::Keychain)),
                Label::ActionSecretsDefaultKeychain,
            ),
            ("file", Setting::DefaultSource(DefaultSource::Kind(SourceKind::File)), Label::ActionSecretsDefaultFile),
            (
                "command",
                Setting::DefaultSource(DefaultSource::Kind(SourceKind::Command)),
                Label::ActionSecretsDefaultCommand,
            ),
            ("env", Setting::DefaultSource(DefaultSource::Kind(SourceKind::Env)), Label::ActionSecretsDefaultEnv),
            (
                "prompt",
                Setting::DefaultSource(DefaultSource::Kind(SourceKind::Prompt)),
                Label::ActionSecretsDefaultPrompt,
            ),
        ]),
    },
    SettingSpec {
        key: "commands.position",
        label: Label::SettingCommandsPosition,
        about: Label::SettingCommandsPositionAbout,
        group: SettingGroup::Display,
        values: Values::Fixed(&[
            ("popup", Setting::CommandsPosition(CommandsPosition::Popup), Label::SettingCommandsPositionPopup),
            ("bottom", Setting::CommandsPosition(CommandsPosition::Bottom), Label::SettingCommandsPositionBottom),
        ]),
    },
    SettingSpec {
        key: "detail_view",
        label: Label::SettingDetailView,
        about: Label::SettingDetailViewAbout,
        group: SettingGroup::Results,
        values: Values::Fixed(&[
            ("panel", Setting::DetailView(DetailView::Panel), Label::SettingDetailViewPanel),
            ("statusbar", Setting::DetailView(DetailView::Statusbar), Label::SettingDetailViewStatusbar),
        ]),
    },
    SettingSpec {
        key: "clipboard",
        label: Label::SettingClipboard,
        about: Label::SettingClipboardAbout,
        group: SettingGroup::Clipboard,
        values: Values::Fixed(&[
            ("auto", Setting::Clipboard(ClipboardSetting::Auto), Label::SettingClipboardAuto),
            ("system", Setting::Clipboard(ClipboardSetting::System), Label::SettingClipboardSystem),
            ("osc52", Setting::Clipboard(ClipboardSetting::Osc52), Label::SettingClipboardOsc52),
        ]),
    },
    SettingSpec {
        key: "copy_header",
        label: Label::SettingCopyHeader,
        about: Label::SettingCopyHeaderAbout,
        group: SettingGroup::Clipboard,
        values: Values::Fixed(&[
            ("auto", Setting::CopyHeader(CopyHeader::Auto), Label::SettingCopyHeaderAuto),
            ("on", Setting::CopyHeader(CopyHeader::On), Label::SettingCopyHeaderOn),
            ("off", Setting::CopyHeader(CopyHeader::Off), Label::SettingCopyHeaderOff),
        ]),
    },
    SettingSpec {
        key: "editor.cursor_shape",
        label: Label::SettingCursorShape,
        about: Label::SettingCursorShapeAbout,
        group: SettingGroup::Editor,
        values: Values::Fixed(&[
            ("on", Setting::CursorShape(CursorShape::On), Label::SettingCursorShapeOn),
            ("off", Setting::CursorShape(CursorShape::Off), Label::SettingCursorShapeOff),
        ]),
    },
    SettingSpec {
        key: "editor.clipboard",
        label: Label::SettingEditorClipboard,
        about: Label::SettingEditorClipboardAbout,
        group: SettingGroup::Editor,
        values: Values::Fixed(&[
            ("on", Setting::EditorClipboard(EditorClipboard::On), Label::SettingEditorClipboardOn),
            ("off", Setting::EditorClipboard(EditorClipboard::Off), Label::SettingEditorClipboardOff),
        ]),
    },
    SettingSpec {
        key: "theme",
        label: Label::SettingTheme,
        about: Label::SettingThemeAbout,
        group: SettingGroup::Display,
        values: Values::Themes,
    },
    SettingSpec {
        key: "editor.format_keyword_case",
        label: Label::SettingFormatKeywordCase,
        about: Label::SettingFormatKeywordCaseAbout,
        group: SettingGroup::Editor,
        values: Values::Fixed(&[
            ("preserve", Setting::FormatCase(KeywordCase::Preserve), Label::SettingFormatKeywordCasePreserve),
            ("upper", Setting::FormatCase(KeywordCase::Upper), Label::SettingFormatKeywordCaseUpper),
            ("lower", Setting::FormatCase(KeywordCase::Lower), Label::SettingFormatKeywordCaseLower),
        ]),
    },
    SettingSpec {
        key: "editor.format_indent",
        label: Label::SettingFormatIndent,
        about: Label::SettingFormatIndentAbout,
        group: SettingGroup::Editor,
        values: Values::Fixed(&[
            ("4", Setting::FormatIndent(FormatIndent::Four), Label::SettingFormatIndentFour),
            ("2", Setting::FormatIndent(FormatIndent::Two), Label::SettingFormatIndentTwo),
        ]),
    },
    SettingSpec {
        key: "editor.auto_pairs",
        label: Label::SettingAutoPairs,
        about: Label::SettingAutoPairsAbout,
        group: SettingGroup::Editor,
        values: Values::Fixed(&[
            ("off", Setting::AutoPairs(AutoPairs::Off), Label::SettingAutoPairsOff),
            ("on", Setting::AutoPairs(AutoPairs::On), Label::SettingAutoPairsOn),
        ]),
    },
    SettingSpec {
        key: "editor.soft_tab_backspace",
        label: Label::SettingSoftTabBackspace,
        about: Label::SettingSoftTabBackspaceAbout,
        group: SettingGroup::Editor,
        values: Values::Fixed(&[
            ("on", Setting::SoftTabBackspace(SoftTabBackspace::On), Label::SettingSoftTabBackspaceOn),
            ("off", Setting::SoftTabBackspace(SoftTabBackspace::Off), Label::SettingSoftTabBackspaceOff),
        ]),
    },
    SettingSpec {
        key: "editor.run_hints",
        label: Label::SettingRunHints,
        about: Label::SettingRunHintsAbout,
        group: SettingGroup::Editor,
        values: Values::Fixed(&[
            ("on", Setting::RunHints(RunHints::On), Label::SettingRunHintsOn),
            ("off", Setting::RunHints(RunHints::Off), Label::SettingRunHintsOff),
        ]),
    },
    SettingSpec {
        key: "secrets.macos_keychain_access",
        label: Label::SettingKeychainAccess,
        about: Label::SettingKeychainAccessAbout,
        group: SettingGroup::Secrets,
        values: Values::Fixed(&[
            ("direct", Setting::KeychainAccess(KeychainAccess::Direct), Label::SettingKeychainAccessDirect),
            (
                "security_tool",
                Setting::KeychainAccess(KeychainAccess::SecurityTool),
                Label::SettingKeychainAccessSecurityTool,
            ),
        ]),
    },
    SettingSpec {
        key: "explorer.size_heat",
        label: Label::SettingSizeHeat,
        about: Label::SettingSizeHeatAbout,
        group: SettingGroup::Display,
        values: Values::Fixed(&[
            ("3", Setting::SizeHeat(SizeHeat::Three), Label::SettingSizeHeatThree),
            ("5", Setting::SizeHeat(SizeHeat::Five), Label::SettingSizeHeatFive),
            ("off", Setting::SizeHeat(SizeHeat::Off), Label::SettingSizeHeatOff),
        ]),
    },
    SettingSpec {
        key: "ui.yank_flash",
        label: Label::SettingYankFlash,
        about: Label::SettingYankFlashAbout,
        group: SettingGroup::Results,
        values: Values::Fixed(&[
            ("on", Setting::YankFlash(YankFlash::On), Label::SettingYankFlashOn),
            ("off", Setting::YankFlash(YankFlash::Off), Label::SettingYankFlashOff),
        ]),
    },
    // `:set` also takes any other duration these two take (`7m`); the screen shows it as is.
    SettingSpec {
        key: "idle_disconnect",
        label: Label::SettingIdleDisconnect,
        about: Label::SettingIdleDisconnectAbout,
        group: SettingGroup::Connections,
        values: Values::Fixed(&[
            ("off", Setting::IdleDisconnect(None), Label::SettingIdleDisconnectOff),
            ("5m", Setting::IdleDisconnect(secs(300)), Label::SettingIdleDisconnectOn),
            ("10m", Setting::IdleDisconnect(secs(600)), Label::SettingIdleDisconnectOn),
            ("30m", Setting::IdleDisconnect(secs(1800)), Label::SettingIdleDisconnectOn),
            ("1h", Setting::IdleDisconnect(secs(3600)), Label::SettingIdleDisconnectOn),
        ]),
    },
    SettingSpec {
        key: "lock_wait",
        label: Label::SettingLockWait,
        about: Label::SettingLockWaitAbout,
        group: SettingGroup::Connections,
        values: Values::Fixed(&[
            ("off", Setting::LockWait(None), Label::SettingLockWaitOff),
            ("5s", Setting::LockWait(secs(5)), Label::SettingLockWaitOn),
            ("10s", Setting::LockWait(secs(10)), Label::SettingLockWaitOn),
            ("30s", Setting::LockWait(secs(30)), Label::SettingLockWaitOn),
            ("1m", Setting::LockWait(secs(60)), Label::SettingLockWaitOn),
        ]),
    },
];

/// A duration setting's value typed for `:set` that is not one of its presets (`7m`, `300s`):
/// the setting it applies, if `spec` is a duration setting and the value is valid for it.
fn duration_setting(spec: &SettingSpec, value: &str) -> Option<Setting> {
    use datarig_core::policy;
    match spec.key {
        "idle_disconnect" => policy::parse_idle_disconnect_text(value).ok().map(Setting::IdleDisconnect),
        "lock_wait" => policy::parse_lock_wait_text(value).ok().map(Setting::LockWait),
        _ => None,
    }
}

impl SettingSpec {
    /// `en|ko|auto`, for messages (the built-in names for the theme).
    pub fn value_list(&self) -> String {
        match self.values {
            Values::Fixed(v) => v.iter().map(|v| v.0).collect::<Vec<_>>().join("|"),
            Values::Themes => crate::theme::NAMES.join("|"),
        }
    }
}

/// What the argument of `:set` asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetValue<'a> {
    Setting(Setting),
    /// `theme=<name>`: the app looks the name up.
    Theme(&'a str),
}

/// Why `:set` could not apply its argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetError {
    /// No `key=value`.
    Usage,
    UnknownKey(String),
    BadValue {
        key: &'static str,
        value: String,
        values: String,
    },
}

/// Split `key=value` (spaces around `=` are allowed).
fn split_setting(arg: &str) -> Option<(&str, &str)> {
    arg.split_once('=').map(|(k, v)| (k.trim(), v.trim()))
}

/// Read the argument of `:set`.
pub fn parse_set(arg: &str) -> Result<SetValue<'_>, SetError> {
    let Some((key, value)) = split_setting(arg) else { return Err(SetError::Usage) };
    if key.is_empty() {
        return Err(SetError::Usage);
    }
    let Some(spec) = SETTINGS.iter().find(|s| s.key == key) else { return Err(SetError::UnknownKey(key.to_string())) };
    if spec.values == Values::Themes && !value.is_empty() {
        return Ok(SetValue::Theme(value));
    }
    let values = spec.values.fixed();
    // `on`, the size colors' value from before there were levels, is `3` (not listed).
    let value_lc = match (spec.key, value.to_ascii_lowercase()) {
        ("explorer.size_heat", v) if v == "on" => "3".to_string(),
        (_, v) => v,
    };
    let found = values.iter().find(|v| v.0 == value_lc).map(|v| v.1).or_else(|| duration_setting(spec, &value_lc));
    found.map(SetValue::Setting).ok_or_else(|| SetError::BadValue {
        key: spec.key,
        value: value.to_string(),
        values: spec.value_list(),
    })
}

/// `language|editor`, for messages.
pub fn setting_keys() -> String {
    SETTINGS.iter().map(|s| s.key).collect::<Vec<_>>().join("|")
}

/// A completion of a command's argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgCompletion {
    /// Index into the profile names given to [`complete_arg`].
    Profile(usize),
    /// Index into the saved query names given to [`complete_arg`].
    Script(usize),
    /// Index into the contexts (`db`, `db.schema`, `.schema`) given to [`complete_arg`].
    Context(usize),
    /// A setting of [`SETTINGS`] (its value still has to be typed).
    SetKey(usize),
    /// A value of a setting: `(setting, value)` indices into [`SETTINGS`].
    SetValue(usize, usize),
    /// A theme: `(setting, name)`, an index into [`SETTINGS`] and one into the theme names given
    /// to [`complete_arg`].
    Theme(usize, usize),
    /// A format of [`super::copy::CopyFormat::MENU`].
    Format(usize),
    /// A format of [`super::copy::CopyFormat::MENU`] and a scope of
    /// [`super::copy::CopyScope::ALL`].
    Scope(usize, usize),
    /// A word of [`THEME_ACTIONS`] (`copy`, `new`).
    ThemeAction(usize),
    /// The theme `:theme copy` copies: an index into the theme names given to [`complete_arg`].
    ThemeFrom(usize),
}

/// The words of `:theme`, each with its label.
pub const THEME_ACTIONS: [(&str, Label); 2] = [("copy", Label::CommandThemeCopy), ("new", Label::CommandThemeNew)];

/// What `:theme` asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeCommand<'a> {
    /// `copy <from> <name>`.
    Copy { from: &'a str, name: &'a str },
    /// `new <name>`.
    New { name: &'a str },
}

/// Read the argument of `:theme`; `None`: it is not `copy <from> <name>` or `new <name>`.
pub fn parse_theme(arg: &str) -> Option<ThemeCommand<'_>> {
    let words: Vec<&str> = arg.split_whitespace().collect();
    // The word in any case, as the completion takes it.
    match (words.first().map(|w| w.to_ascii_lowercase()).as_deref(), &words[..]) {
        (Some("copy"), [_, from, name]) => Some(ThemeCommand::Copy { from, name }),
        (Some("new"), [_, name]) => Some(ThemeCommand::New { name }),
        _ => None,
    }
}

/// `name` as `:use` takes it (what [`ident`] reads back as `name`): as it is when it is lower-case
/// ASCII letters, digits and `_` (not starting with a digit), else quoted with `"` doubled. This
/// is the command's own syntax, not SQL: keywords need no quotes.
pub(super) fn context_name(name: &str) -> String {
    let plain = !name.is_empty()
        && name.chars().all(|c| c == '_' || c.is_ascii_lowercase() || c.is_ascii_digit())
        && !name.starts_with(|c: char| c.is_ascii_digit());
    if plain { name.to_string() } else { format!("\"{}\"", name.replace('"', "\"\"")) }
}

/// An identifier of `:use` as typed: `"quoted"` keeps its dots and case (`""` is a quote),
/// anything else is taken as written. `Err`: an unterminated quote.
fn ident(s: &str) -> Result<(String, &str), ()> {
    if let Some(rest) = s.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = rest.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c == '"' {
                if chars.peek().is_some_and(|(_, n)| *n == '"') {
                    chars.next();
                    out.push('"');
                } else {
                    return Ok((out, &rest[i + 1..]));
                }
            } else {
                out.push(c);
            }
        }
        return Err(());
    }
    // Unquoted, a name folds to lower case as in SQL: `Shop` is `shop`.
    let end = s.find('.').unwrap_or(s.len());
    Ok((s[..end].to_ascii_lowercase(), &s[end..]))
}

/// Where `:tabmove <arg>` puts the tab at `from` of `len` tabs, as Vim does: `N` after tab `N`
/// counted before the move (`0`: first), `+N` / `-N` that many places right or left (`+` and
/// `-` alone: one), nothing or `$` last. Past an end: that end. `None`: not one of these, or no
/// tab.
pub fn tabmove_target(arg: &str, from: usize, len: usize) -> Option<usize> {
    let last = len.checked_sub(1)?;
    // Digits only; too many of them count as a place past every tab.
    let number = |s: &str| -> Option<usize> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().unwrap_or(usize::MAX))
    };
    let count = |s: &str| if s.is_empty() { Some(1) } else { number(s) };
    match arg.trim() {
        "" | "$" => Some(last),
        a if a.starts_with('+') => count(&a[1..]).map(|n| from.saturating_add(n).min(last)),
        a if a.starts_with('-') => count(&a[1..]).map(|n| from.saturating_sub(n)),
        a => number(a).map(|n| super::tab_drag::slot_index(from, n.min(len))),
    }
}

/// The argument of `:use`: `(database, schema)`, each `None` when left out (`db`: its default
/// schema; `.schema`: the tab's database). Unquoted names fold to lower case, `"Quoted"` ones
/// keep their case (and dots). `None`: it cannot be read.
pub fn parse_context(arg: &str) -> Option<(Option<String>, Option<String>)> {
    let arg = arg.trim();
    let (db, rest) = if arg.starts_with('.') { (None, arg) } else { ident(arg).map(|(d, r)| (Some(d), r)).ok()? };
    let schema = match rest.strip_prefix('.') {
        None if rest.is_empty() => None,
        None => return None,
        Some(r) => {
            let (s, tail) = ident(r).ok()?;
            if !tail.is_empty() || s.is_empty() {
                return None;
            }
            Some(s)
        }
    };
    if db.as_deref() == Some("") {
        return None;
    }
    Some((db, schema))
}

/// Rank `candidates` against `typed`: exact first, then prefixes, then fuzzy matches; ties keep
/// the given order. Case-insensitive.
fn rank<'a>(typed: &str, candidates: impl Iterator<Item = (usize, &'a str)>) -> Vec<usize> {
    let t = typed.to_lowercase();
    let mut hits: Vec<(u8, i32, usize)> = candidates
        .filter_map(|(i, c)| {
            let c = c.to_lowercase();
            if c == t {
                Some((0, 0, i))
            } else if c.starts_with(&t) {
                Some((1, 0, i))
            } else {
                super::action::fuzzy_score(&t, &c).map(|s| (2, -s, i))
            }
        })
        .collect();
    hits.sort();
    hits.into_iter().map(|(_, _, i)| i).collect()
}

/// The completions of the argument `arg` of a command taking `kind`, best first. `names`
/// are the profile names ([`ArgKind::Profile`]), the saved queries ([`ArgKind::Script`]), the
/// contexts ([`ArgKind::Context`]) or the theme names ([`ArgKind::Setting`], [`ArgKind::Theme`]).
pub fn complete_arg(kind: ArgKind, arg: &str, names: &[&str]) -> Vec<ArgCompletion> {
    match kind {
        ArgKind::Profile => {
            rank(arg, names.iter().copied().enumerate()).into_iter().map(ArgCompletion::Profile).collect()
        }
        ArgKind::Script => {
            rank(arg, names.iter().copied().enumerate()).into_iter().map(ArgCompletion::Script).collect()
        }
        ArgKind::Context => {
            rank(arg, names.iter().copied().enumerate()).into_iter().map(ArgCompletion::Context).collect()
        }
        // A new name, or any object's: nothing to pick from (`analyze` is the only word; a
        // tab's place is a number).
        ArgKind::NewScript | ArgKind::Object | ArgKind::Analyze | ArgKind::TabPosition | ArgKind::Export => Vec::new(),
        ArgKind::Format => {
            use super::copy::{CopyFormat, CopyScope};
            let names = CopyFormat::MENU.map(|f| f.name());
            match arg.split_once(char::is_whitespace) {
                // A format typed: its scopes.
                Some((format, scope)) => {
                    let Some(f) = CopyFormat::parse(format).and_then(|f| CopyFormat::MENU.iter().position(|m| *m == f))
                    else {
                        return Vec::new();
                    };
                    let scope = scope.trim().to_lowercase();
                    (0..CopyScope::ALL.len())
                        .filter(|&i| CopyScope::ALL[i].name().starts_with(&scope))
                        .map(|i| ArgCompletion::Scope(f, i))
                        .collect()
                }
                None => rank(arg, names.iter().copied().enumerate())
                    .into_iter()
                    .filter(|&i| names[i].starts_with(&arg.to_lowercase()))
                    .map(ArgCompletion::Format)
                    .collect(),
            }
        }
        ArgKind::Theme => match arg.split_once(char::is_whitespace) {
            // `copy ` and the start of a theme name: the names (the new name has nothing to pick).
            Some((word, from))
                if word.eq_ignore_ascii_case("copy") && !from.trim_start().contains(char::is_whitespace) =>
            {
                let from = from.trim_start();
                rank(from, names.iter().copied().enumerate())
                    .into_iter()
                    .filter(|&i| names[i].starts_with(from))
                    .map(ArgCompletion::ThemeFrom)
                    .collect()
            }
            Some(_) => Vec::new(),
            None => (0..THEME_ACTIONS.len())
                .filter(|&i| THEME_ACTIONS[i].0.starts_with(&arg.to_lowercase()))
                .map(ArgCompletion::ThemeAction)
                .collect(),
        },
        ArgKind::Setting => match split_setting(arg) {
            Some((key, value)) => match SETTINGS.iter().position(|s| s.key == key) {
                Some(k) if SETTINGS[k].values == Values::Themes => rank(value, names.iter().copied().enumerate())
                    .into_iter()
                    .filter(|&v| names[v].starts_with(value))
                    .map(|v| ArgCompletion::Theme(k, v))
                    .collect(),
                Some(k) => {
                    let values = SETTINGS[k].values.fixed();
                    rank(value, values.iter().map(|v| v.0).enumerate())
                        .into_iter()
                        .filter(|&v| value.is_empty() || values[v].0.starts_with(&value.to_lowercase()))
                        .map(|v| ArgCompletion::SetValue(k, v))
                        .collect()
                }
                None => Vec::new(),
            },
            None => rank(arg, SETTINGS.iter().map(|s| s.key).enumerate())
                .into_iter()
                .filter(|&k| SETTINGS[k].key.starts_with(&arg.to_lowercase()))
                .map(ArgCompletion::SetKey)
                .collect(),
        },
    }
}

#[cfg(test)]
mod tests;
