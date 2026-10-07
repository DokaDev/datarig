//! Connection profile form state: a large dialog with three
//! sections. **Basic**: the driver (only registered drivers can be picked), the required fields
//! first, the password storage selector whose field follows the chosen source, and a DSN
//! kept in two-way sync with the fields. **SSH**: the tunnel: none (the default), a tunnel
//! preset, or one of this profile only (its bastion's fields), which "save as tunnel preset"
//! turns into a new preset when the form is saved. **Advanced**: SSL mode and the server-side
//! statement cache (PostgreSQL), the server's public key file and key retrieval (MySQL),
//! policy name, color, icon and folder. No I/O here; `App` persists and tests.
//!
//! The same form edits a tunnel preset ([`FormKind::Tunnel`]): its name and the SSH section's
//! bastion fields, nothing else.

use crate::widgets::text_input::{InputResult, TextInput};
use datarig_core::i18n::{Label, Msg};
use datarig_core::profile::color::{self, ProfileColor};
use datarig_core::profile::dsn::{self, Dsn, DsnError, Scheme};
use datarig_core::profile::ssh::{SshAuth, SshSettings};
use datarig_core::profile::tunnel::{self, TunnelId, TunnelPreset};
use datarig_core::profile::{ConnectionConfig, SSL_MODES};
use datarig_core::secret::command;
use datarig_core::secret::source::valid_env_name;
use datarig_core::secret::{PasswordSource, SourceKind};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The drivers the form lists, in order: `(driver name, display name)`. Only the ones the
/// app has a driver for can be picked; the others are shown as not yet supported.
pub const DRIVERS: [(&str, &str); 4] =
    [("postgres", "PostgreSQL"), ("mysql", "MySQL"), ("redis", "Redis"), ("elasticsearch", "Elasticsearch")];

/// The form's sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Basic,
    Ssh,
    Advanced,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Basic, Section::Ssh, Section::Advanced];

    pub fn label(self) -> Label {
        match self {
            Section::Basic => Label::FormSectionBasic,
            Section::Ssh => Label::FormSectionSsh,
            Section::Advanced => Label::FormSectionAdvanced,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The driver selector (Basic, first).
    Driver,
    Name,
    Host,
    Port,
    User,
    /// The password storage selector.
    Source,
    /// The password (keychain and file sources).
    Password,
    /// The command of the `command` source.
    Command,
    /// The variable name of the `env` source.
    Env,
    Database,
    Dsn,
    SslMode,
    /// The server-side statement cache, on or off (Advanced).
    StatementCache,
    /// MySQL: the server's public key file (Advanced).
    ServerKey,
    /// MySQL: a direct login may ask the server for its public key, or not (Advanced).
    KeyRetrieval,
    /// The policy name (Advanced).
    Policy,
    /// Color, icon and folder: pickers (Advanced).
    Color,
    Icon,
    Folder,
    /// The SSH tunnel: on or off, then the bastion.
    SshEnabled,
    SshHost,
    SshPort,
    SshUser,
    /// How to log in (a selector).
    SshAuth,
    SshKeyFile,
    /// Where the passphrase or password comes from (a selector).
    SshSource,
    /// The passphrase or password (keychain and file sources).
    SshSecret,
    SshCommand,
    SshEnv,
    SshKeepalive,
    SshTimeout,
    Test,
    Save,
    Cancel,
}

/// The fields of each section in the order `Tab` visits them, before the ones the source hides
/// are left out ([`ProfileForm::fields`]). The buttons follow in both.
pub const BASIC: [Field; 10] = [
    Field::Driver,
    Field::Name,
    Field::Host,
    Field::Port,
    Field::User,
    Field::Source,
    Field::Password,
    Field::Command,
    Field::Env,
    Field::Database,
];
pub const ADVANCED: [Field; 8] = [
    Field::SslMode,
    Field::StatementCache,
    Field::ServerKey,
    Field::KeyRetrieval,
    Field::Policy,
    Field::Color,
    Field::Icon,
    Field::Folder,
];
pub const BUTTONS: [Field; 3] = [Field::Test, Field::Save, Field::Cancel];
pub const SSH: [Field; 12] = [
    Field::SshEnabled,
    Field::SshHost,
    Field::SshPort,
    Field::SshUser,
    Field::SshAuth,
    Field::SshKeyFile,
    Field::SshSource,
    Field::SshSecret,
    Field::SshCommand,
    Field::SshEnv,
    Field::SshKeepalive,
    Field::SshTimeout,
];

/// Every field with a label, for the width of the label column.
pub const FIELDS: [Field; 19] = [
    Field::Driver,
    Field::Name,
    Field::Host,
    Field::Port,
    Field::User,
    Field::Source,
    Field::Password,
    Field::Command,
    Field::Env,
    Field::Database,
    Field::Dsn,
    Field::SslMode,
    Field::StatementCache,
    Field::ServerKey,
    Field::KeyRetrieval,
    Field::Policy,
    Field::Color,
    Field::Icon,
    Field::Folder,
];

/// The field under the storage selector for `source` (`None` for `prompt`).
pub fn secret_field(source: SourceKind) -> Option<Field> {
    match source {
        SourceKind::Keychain | SourceKind::File => Some(Field::Password),
        SourceKind::Command => Some(Field::Command),
        SourceKind::Env => Some(Field::Env),
        SourceKind::Prompt => None,
    }
}

/// Short name of a source for the form's selector.
pub fn source_choice(kind: SourceKind) -> Label {
    match kind {
        SourceKind::Keychain => Label::FormSourceKeychain,
        SourceKind::File => Label::FormSourceFile,
        SourceKind::Command => Label::FormSourceCommand,
        SourceKind::Env => Label::FormSourceEnv,
        SourceKind::Prompt => Label::FormSourcePrompt,
    }
}

/// Name of a source in messages ("the OS keychain", "the secrets file", …).
pub fn source_name(kind: SourceKind) -> Label {
    match kind {
        SourceKind::Keychain => Label::SourceKeychain,
        SourceKind::File => Label::SourceFile,
        SourceKind::Command => Label::SourceCommand,
        SourceKind::Env => Label::SourceEnv,
        SourceKind::Prompt => Label::SourcePrompt,
    }
}

impl Field {
    pub fn label(self) -> Label {
        match self {
            Field::Driver => Label::FormFieldDriver,
            Field::Name => Label::FormFieldName,
            Field::Host => Label::FormFieldHost,
            Field::Port => Label::FormFieldPort,
            Field::User => Label::FormFieldUser,
            Field::Source => Label::FormFieldSource,
            Field::Password => Label::FormFieldPassword,
            Field::Command => Label::FormFieldCommand,
            Field::Env => Label::FormFieldEnv,
            Field::Database => Label::FormFieldDatabase,
            Field::SslMode => Label::FormFieldSslmode,
            Field::Dsn => Label::FormFieldDsn,
            Field::StatementCache => Label::FormFieldStatementCache,
            Field::ServerKey => Label::FormFieldServerKey,
            Field::KeyRetrieval => Label::FormFieldKeyRetrieval,
            Field::Policy => Label::FormFieldPolicy,
            Field::Color => Label::FormFieldColor,
            Field::Icon => Label::FormFieldIcon,
            Field::Folder => Label::FormFieldFolder,
            Field::SshEnabled => Label::FormFieldSshEnabled,
            Field::SshHost => Label::FormFieldSshHost,
            Field::SshPort => Label::FormFieldPort,
            Field::SshUser => Label::FormFieldUser,
            Field::SshAuth => Label::FormFieldSshAuth,
            Field::SshKeyFile => Label::FormFieldSshKeyFile,
            Field::SshSource => Label::FormFieldSource,
            Field::SshSecret => Label::FormFieldSshSecret,
            Field::SshCommand => Label::FormFieldCommand,
            Field::SshEnv => Label::FormFieldEnv,
            Field::SshKeepalive => Label::FormFieldSshKeepalive,
            Field::SshTimeout => Label::FormFieldSshTimeout,
            Field::Test => Label::FormButtonTest,
            Field::Save => Label::FormButtonSave,
            Field::Cancel => Label::FormButtonCancel,
        }
    }

    pub fn is_button(self) -> bool {
        matches!(self, Field::Test | Field::Save | Field::Cancel)
    }

    /// The section the field is in (buttons: both).
    pub fn section(self) -> Option<Section> {
        if BASIC.contains(&self) || self == Field::Dsn {
            Some(Section::Basic)
        } else if SSH.contains(&self) {
            Some(Section::Ssh)
        } else if ADVANCED.contains(&self) {
            Some(Section::Advanced)
        } else {
            None
        }
    }

    /// A picker field: `Enter` opens its list.
    pub fn is_picker(self) -> bool {
        matches!(self, Field::Color | Field::Icon | Field::Folder)
    }
}

/// What the SSH section's picker of a profile form has picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SshChoice {
    /// No tunnel: the database directly.
    Off,
    /// The tunnel preset of this name.
    Preset(String),
    /// A tunnel of this profile only (`[connections.ssh]`).
    Inline,
}

/// What the form edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormKind {
    Profile,
    /// A tunnel preset with this id (a new one for a new preset or a copy); `editing`: one that
    /// exists.
    Tunnel {
        id: TunnelId,
        editing: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldError {
    Required,
    Port,
    NameTaken,
    /// Not a name a tunnel preset can have (blanks around it, a control character, too long).
    TunnelName,
    /// Not an environment variable name.
    EnvName,
    /// The command cannot be split (an unclosed quote).
    CommandSyntax,
    /// Seconds: a whole number is needed.
    Seconds,
    /// A file path that has no root and does not start with `~/`.
    AbsolutePath,
}

impl FieldError {
    pub fn label(self) -> Label {
        match self {
            FieldError::Required => Label::ValidateRequired,
            FieldError::Port => Label::ValidatePort,
            FieldError::NameTaken => Label::ValidateNameTaken,
            FieldError::TunnelName => Label::ValidateTunnelName,
            FieldError::EnvName => Label::ValidateEnvName,
            FieldError::CommandSyntax => Label::ValidateCommandSyntax,
            FieldError::Seconds => Label::ValidateSeconds,
            FieldError::AbsolutePath => Label::ValidateServerKeyPath,
        }
    }
}

/// Why the DSN field could not be applied to the fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DsnProblem {
    Parse(DsnError),
    Param(String),
    /// A parameter in a `mysql://` URL, which takes none.
    MySqlParam(String),
    SslMode(String),
}

impl DsnProblem {
    pub fn message(&self) -> Msg {
        match self {
            DsnProblem::Parse(e) => e.message(),
            DsnProblem::Param(p) => Msg::DsnErrParam { name: p.clone() },
            DsnProblem::MySqlParam(p) => Msg::DsnErrParamMysql { name: p.clone() },
            DsnProblem::SslMode(v) => Msg::DsnErrSslmode { value: v.clone() },
        }
    }
}

/// What a click on the form hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormHit {
    /// A section's tab.
    Section(Section),
    /// A field's line: the focus goes there.
    Field(Field),
    /// A text field's input: the focus, and the cursor under the pointer.
    Input(Field),
    /// One of the values a field shows side by side (driver, storage, SSH login), by index.
    Choice(Field, usize),
    /// The `‹` or `›` of a `‹ value ›` selector: the previous or the next value.
    Prev(Field),
    Next(Field),
    /// A selector's value: the next one, or the list of a picker.
    Value(Field),
    /// Test, Save or Cancel.
    Button(Field),
    /// The key file field's `[…]`.
    KeyFile,
    /// "Save as tunnel preset".
    SaveAsPreset,
}

impl FormHit {
    /// A button: drawn highlighted under the pointer.
    pub fn is_button(self) -> bool {
        matches!(self, FormHit::Button(_) | FormHit::KeyFile | FormHit::SaveAsPreset)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormOutcome {
    None,
    Save,
    Test,
    Cancel,
    /// Open the list of a picker field (color, icon, folder).
    Choose(Field),
}

pub struct ProfileForm {
    /// A profile, or a tunnel preset.
    pub kind: FormKind,
    /// Index in the profile list being edited; `None` for a new or duplicated profile.
    pub editing: Option<usize>,
    /// Name before editing (the form's title).
    pub original_name: Option<String>,
    /// The profile the form started from: its id, driver, display attributes (folder, color,
    /// icon, policy) and file origin, which the form keeps as they are.
    base: ConnectionConfig,
    pub name: TextInput,
    pub host: TextInput,
    pub port: TextInput,
    pub user: TextInput,
    pub password: TextInput,
    pub database: TextInput,
    pub dsn: TextInput,
    pub sslmode: usize,
    /// Keep prepared statements on the server between transactions (off behind a pooler in
    /// transaction mode without prepared statement support).
    pub statement_cache: bool,
    /// MySQL: the server's public key file (empty: none).
    pub server_key: TextInput,
    /// MySQL: a direct login to another machine may ask the server for its public key.
    pub key_retrieval: bool,
    /// Index into [`DRIVERS`].
    pub driver: usize,
    /// Which [`DRIVERS`] can be picked (the app has a driver for them).
    pub drivers_enabled: [bool; 4],
    pub section: Section,
    /// The policy name (empty: `default`).
    pub policy: TextInput,
    /// The color set (`None`: automatic from the name).
    pub color: Option<String>,
    /// The icon set (`None`: the driver's).
    pub icon: Option<String>,
    /// The folder (`None`: the top level).
    pub folder: Option<String>,
    /// Every folder there is, for the folder picker.
    pub folders: Vec<String>,
    /// Where the password comes from (the storage selector).
    pub source: SourceKind,
    /// The command of the `command` source.
    pub command: TextInput,
    /// The variable name of the `env` source.
    pub env: TextInput,
    /// The keychain works; otherwise the selector skips it and says why.
    pub keychain_ok: bool,
    /// The stored password could not be read when the form opened (e.g. an insecure secrets
    /// file): an empty password field then keeps the stored one instead of removing it.
    pub password_unread: bool,
    /// The keychain is being read for this profile's password: the field is
    /// filled when it answers.
    pub reading: Option<datarig_core::profile::ProfileId>,
    /// A keychain write of the save is running: the form waits for it (at most the keychain's
    /// time limit).
    pub saving: bool,
    /// The SSH tunnel. `ssh_enabled`: the profile's own (the bastion fields below);
    /// `ssh_had`: the profile has settings of its own (kept when turned off or when a preset
    /// is picked); a profile that never had any gets none until they are turned on.
    pub ssh_enabled: bool,
    ssh_had: bool,
    /// The tunnel preset picked (its name); `None`: none. Never together with `ssh_enabled`.
    pub ssh_preset: Option<String>,
    /// The saved presets the picker offers.
    pub presets: Vec<TunnelPreset>,
    /// "Save as tunnel preset": a new preset of this name made of the bastion fields when the
    /// form is saved (the profile then names it, and its own settings move into it).
    pub new_preset: Option<String>,
    /// The profile named a preset and had its own tunnel on (an error of the profile): the form
    /// shows the preset picked, its own tunnel off, and says so; saving keeps what is picked.
    pub ssh_both: bool,
    pub ssh_host: TextInput,
    pub ssh_port: TextInput,
    pub ssh_user: TextInput,
    pub ssh_auth: SshAuth,
    pub ssh_key: TextInput,
    pub ssh_source: SourceKind,
    /// A passphrase or password typed here (empty: keep what is stored).
    pub ssh_secret: TextInput,
    pub ssh_command: TextInput,
    pub ssh_env: TextInput,
    pub ssh_keepalive: TextInput,
    pub ssh_timeout: TextInput,
    /// What the key file picker said about the file it picked: others may read
    /// it, a PuTTY key, no file there. Editing the field clears it.
    pub ssh_key_note: Option<Msg>,
    /// What was drawn where (mouse), kept by the renderer; a later entry lies on an earlier one.
    pub hits: Vec<(ratatui::layout::Rect, FormHit)>,
    /// The button under the pointer (drawn highlighted; the focus does not move to it).
    pub hover: Option<FormHit>,
    /// The button a press armed (it acts on the release).
    pub press: super::overlay::Press<FormHit>,
    pub focus: Field,
    pub dsn_problem: Option<DsnProblem>,
    /// Save was attempted: "required" errors are shown from now on.
    pub attempted: bool,
}

impl ProfileForm {
    pub fn new_profile() -> Self {
        let c = ConnectionConfig { host: "localhost".into(), sslmode: "prefer".into(), ..ConnectionConfig::default() };
        Self::from_profile(&c, String::new(), None)
    }

    /// The id of the profile the form edits (a new one for a new profile or a copy).
    pub fn profile_id(&self) -> datarig_core::profile::ProfileId {
        self.base.id
    }

    /// A new profile whose storage selector starts on `source`; `keychain_ok` says whether
    /// the keychain can be picked.
    pub fn new_with_source(source: SourceKind, keychain_ok: bool) -> Self {
        let mut f = Self::new_profile();
        f.source = source;
        f.keychain_ok = keychain_ok;
        f
    }

    /// A form for `c` (`editing`: its index in the list; `None` for a new profile or a copy,
    /// which must come with its own id).
    pub fn from_profile(c: &ConnectionConfig, password: String, editing: Option<usize>) -> Self {
        let mut base = c.clone();
        base.password.clear();
        if editing.is_none() {
            base.origin = None;
        }
        let mut f = Self {
            kind: FormKind::Profile,
            editing,
            original_name: editing.map(|_| c.name.clone()),
            base,
            name: TextInput::new(&c.name),
            host: TextInput::new(&c.host),
            port: TextInput::new(&c.port.to_string()),
            user: TextInput::new(&c.user),
            password: TextInput::new(&password),
            database: TextInput::new(&c.database),
            dsn: TextInput::new(""),
            sslmode: SSL_MODES.iter().position(|m| *m == c.sslmode).unwrap_or(0),
            statement_cache: c.statement_cache,
            server_key: TextInput::new(c.server_public_key_file.as_deref().unwrap_or("")),
            key_retrieval: c.allow_public_key_retrieval,
            driver: driver_index(&c.driver),
            drivers_enabled: [true, false, false, false],
            section: Section::Basic,
            policy: TextInput::new(c.policy.as_deref().unwrap_or("")),
            color: c.color.clone(),
            icon: c.icon.clone(),
            folder: c.folder.clone(),
            folders: Vec::new(),
            source: c.source().kind(),
            command: TextInput::new(c.password_command.as_deref().unwrap_or("")),
            env: TextInput::new(c.password_env.as_deref().unwrap_or("")),
            keychain_ok: true,
            password_unread: false,
            reading: None,
            saving: false,
            ssh_enabled: c.ssh.as_ref().is_some_and(|s| s.enabled),
            ssh_had: c.ssh.is_some(),
            ssh_preset: c.tunnel.clone(),
            presets: Vec::new(),
            new_preset: None,
            ssh_both: false,
            ssh_host: TextInput::new(c.ssh.as_ref().map_or("", |s| s.host.as_str())),
            ssh_port: TextInput::new(&c.ssh.as_ref().map_or(22, |s| s.port).to_string()),
            ssh_user: TextInput::new(c.ssh.as_ref().map_or("", |s| s.user.as_str())),
            ssh_auth: c.ssh.as_ref().map_or(SshAuth::Key, |s| s.auth),
            ssh_key: TextInput::new(c.ssh.as_ref().and_then(|s| s.key_file.as_deref()).unwrap_or("")),
            ssh_source: c.ssh.as_ref().map_or(SourceKind::Keychain, |s| s.source().kind()),
            ssh_secret: TextInput::default(),
            ssh_command: TextInput::new(c.ssh.as_ref().and_then(|s| s.secret_command.as_deref()).unwrap_or("")),
            ssh_env: TextInput::new(c.ssh.as_ref().and_then(|s| s.secret_env.as_deref()).unwrap_or("")),
            ssh_keepalive: TextInput::new(
                &c.ssh.as_ref().and_then(|s| s.keepalive).map(|k| k.to_string()).unwrap_or_default(),
            ),
            ssh_timeout: TextInput::new(
                &c.ssh.as_ref().and_then(|s| s.timeout).map(|k| k.to_string()).unwrap_or_default(),
            ),
            ssh_key_note: None,
            hits: Vec::new(),
            hover: None,
            press: Default::default(),
            focus: Field::Name,
            dsn_problem: None,
            attempted: false,
        };
        if c.tunnel.is_some() && f.ssh_enabled {
            f.ssh_enabled = false;
            f.ssh_both = true;
        }
        match &c.dsn {
            // A DSN that could not be converted at load time: show it and its problem.
            Some(raw) => {
                f.dsn.set(raw);
                f.sync_fields();
            }
            None => f.sync_dsn(),
        }
        f
    }

    /// A form for tunnel preset `p` (`None`: a new one; `copy`: a new one with `p`'s settings
    /// and `name`).
    pub fn for_tunnel(p: Option<&TunnelPreset>, copy: Option<String>) -> Self {
        let settings =
            p.map(|p| p.settings.clone()).unwrap_or_else(|| SshSettings { enabled: true, ..SshSettings::default() });
        let name = copy.clone().or_else(|| p.map(|p| p.name.clone())).unwrap_or_default();
        let c = ConnectionConfig {
            name,
            ssh: Some(SshSettings { enabled: true, ..settings }),
            ..ConnectionConfig::default()
        };
        let editing = p.is_some() && copy.is_none();
        let mut f = Self::from_profile(&c, String::new(), None);
        f.kind = FormKind::Tunnel { id: p.filter(|_| editing).map_or_else(TunnelId::new, |p| p.id), editing };
        f.original_name = p.filter(|_| editing).map(|p| p.name.clone());
        f.section = Section::Ssh;
        f.focus = Field::Name;
        f
    }

    /// The form edits a tunnel preset.
    pub fn is_tunnel(&self) -> bool {
        matches!(self.kind, FormKind::Tunnel { .. })
    }

    /// The preset the tunnel form describes (its typed secret aside).
    pub fn to_preset(&self) -> Option<TunnelPreset> {
        let FormKind::Tunnel { id, .. } = self.kind else { return None };
        let name = self.name.text().trim().to_string();
        let origin = self.original_name.clone();
        Some(TunnelPreset { id, name, settings: SshSettings { enabled: true, ..self.ssh_settings() }, origin })
    }

    /// What the SSH section's picker offers, in order: none, each preset (the one the profile
    /// names stays there even when no preset has that name, and a preset this form makes is
    /// there too), then the profile's own tunnel.
    pub fn ssh_choices(&self) -> Vec<SshChoice> {
        let mut names: Vec<String> = self.presets.iter().map(|p| p.name.clone()).collect();
        for extra in [self.base.tunnel.clone(), self.new_preset.clone()].into_iter().flatten() {
            if !names.contains(&extra) {
                names.push(extra);
            }
        }
        let mut out = vec![SshChoice::Off];
        out.extend(names.into_iter().map(SshChoice::Preset));
        out.push(SshChoice::Inline);
        out
    }

    /// The picker's choice.
    pub fn ssh_choice(&self) -> SshChoice {
        match (&self.ssh_preset, self.ssh_enabled) {
            (Some(p), _) => SshChoice::Preset(p.clone()),
            (None, true) => SshChoice::Inline,
            (None, false) => SshChoice::Off,
        }
    }

    fn set_ssh_choice(&mut self, c: SshChoice) {
        // Picked here: no longer both.
        self.ssh_both = false;
        match c {
            SshChoice::Off => (self.ssh_preset, self.ssh_enabled) = (None, false),
            SshChoice::Preset(p) => (self.ssh_preset, self.ssh_enabled) = (Some(p), false),
            SshChoice::Inline => (self.ssh_preset, self.ssh_enabled) = (None, true),
        }
    }

    fn cycle_ssh_choice(&mut self, d: isize) {
        let all = self.ssh_choices();
        let i = all.iter().position(|c| *c == self.ssh_choice()).unwrap_or(0) as isize;
        let next = all[(i + d).rem_euclid(all.len() as isize) as usize].clone();
        self.set_ssh_choice(next);
    }

    /// The preset picked, when it is a saved one.
    pub fn picked_preset(&self) -> Option<&TunnelPreset> {
        let name = self.ssh_preset.as_deref()?;
        self.presets.iter().find(|p| p.name == name)
    }

    /// The preset picked is the one this form makes ("save as tunnel preset").
    pub fn new_preset_picked(&self) -> bool {
        self.new_preset.is_some() && self.new_preset == self.ssh_preset
    }

    /// "Save as tunnel preset" named `name`: the picker shows the preset the save makes of the
    /// bastion fields.
    pub fn save_as_preset(&mut self, name: &str) {
        self.new_preset = Some(name.to_string());
        self.set_ssh_choice(SshChoice::Preset(name.to_string()));
    }

    /// Put the focus on field `f`, in its section.
    pub fn focus_field(&mut self, f: Field) {
        if let Some(s) = f.section() {
            self.section = s;
        }
        self.focus = f;
    }

    /// A new profile from a pasted connection URL: the fields, the password (out of
    /// the DSN) and a name from the database or the host that `taken` does not use.
    pub fn set_dsn(&mut self, text: &str, taken: impl Fn(&str) -> bool) {
        self.dsn.set(text.trim());
        self.sync_fields();
        let base = [self.database.text(), self.host.text()].into_iter().find(|t| !t.trim().is_empty()).unwrap_or("db");
        let base = base.trim().to_string();
        let name = if taken(&base) { copy_name(&base, &taken) } else { base };
        self.name.set(&name);
    }

    fn port_value(&self) -> Option<u16> {
        self.port.text().trim().parse::<u16>().ok().filter(|p| *p > 0)
    }

    /// The URL scheme of the driver picked: what the DSN field is written in.
    pub fn scheme(&self) -> Scheme {
        Scheme::of_driver(DRIVERS[self.driver].0)
    }

    /// The driver picked is a MySQL one: the PostgreSQL-only settings (SSL mode, the
    /// statement cache) are not shown, MySQL's (the server key file, key retrieval) are.
    pub fn is_mysql(&self) -> bool {
        self.scheme() == Scheme::MySql
    }

    /// The profile would connect to a MySQL server on another machine directly, unencrypted
    /// (see `ConnectionConfig::mysql_unencrypted`).
    pub fn unencrypted(&self) -> bool {
        !self.is_tunnel() && self.to_profile().mysql_unencrypted()
    }

    /// Pick driver `i`: a port still at the old driver's default moves to the new one's, and
    /// the DSN is written in the new driver's scheme.
    fn set_driver(&mut self, i: usize) {
        let before = self.scheme();
        self.driver = i;
        let after = self.scheme();
        if before != after && self.port_value() == Some(before.default_port()) {
            self.port.set(&after.default_port().to_string());
        }
        self.sync_dsn();
    }

    /// Fields -> DSN (never includes the password).
    fn sync_dsn(&mut self) {
        let scheme = self.scheme();
        let d = Dsn {
            scheme,
            user: self.user.text().to_string(),
            password: None,
            host: self.host.text().trim().to_string(),
            port: self.port_value(),
            database: self.database.text().to_string(),
            // MySQL connections have no TLS settings yet.
            params: match scheme {
                Scheme::Postgres => vec![("sslmode".into(), SSL_MODES[self.sslmode].to_string())],
                Scheme::MySql => Vec::new(),
            },
        };
        self.dsn.set(&dsn::format(&d));
        self.dsn_problem = None;
    }

    /// DSN -> fields. On error the fields are left untouched and the problem is shown.
    fn sync_fields(&mut self) {
        let d = match dsn::parse(self.dsn.text()) {
            Ok(d) => d,
            Err(e) => {
                self.dsn_problem = Some(DsnProblem::Parse(e));
                return;
            }
        };
        // The URL's parameters first: one the fields cannot hold leaves everything as it is
        // (the driver too).
        if d.scheme == Scheme::MySql
            && let Some((k, _)) = d.params.first()
        {
            self.dsn_problem = Some(DsnProblem::MySqlParam(k.clone()));
            return;
        }
        let mut sslmode = None;
        for (k, v) in &d.params {
            if k != "sslmode" {
                self.dsn_problem = Some(DsnProblem::Param(k.clone()));
                return;
            }
            match SSL_MODES.iter().position(|m| m == v) {
                Some(i) => sslmode = Some(i),
                None => {
                    self.dsn_problem = Some(DsnProblem::SslMode(v.clone()));
                    return;
                }
            }
        }
        // A URL of the other database picks its driver, when the app has one.
        if d.scheme != self.scheme() {
            let name = match d.scheme {
                Scheme::Postgres => "postgres",
                Scheme::MySql => "mysql",
            };
            match DRIVERS.iter().position(|x| x.0 == name).filter(|i| self.drivers_enabled[*i]) {
                Some(i) => self.driver = i,
                None => {
                    self.dsn_problem = Some(DsnProblem::Parse(DsnError::Scheme));
                    return;
                }
            }
        }
        self.dsn_problem = None;
        self.host.set(&d.host);
        self.port.set(&d.port.unwrap_or(d.scheme.default_port()).to_string());
        self.user.set(&d.user);
        self.database.set(&d.database);
        if let Some(i) = sslmode {
            self.sslmode = i;
        }
        if let Some(pw) = &d.password {
            // Move a pasted password into the password field and out of the DSN text.
            self.password.set(pw);
            self.dsn.set(&dsn::format(&Dsn { password: None, ..d }));
        }
    }

    pub fn input_mut(&mut self, f: Field) -> Option<&mut TextInput> {
        Some(match f {
            Field::Policy => &mut self.policy,
            Field::Name => &mut self.name,
            Field::Host => &mut self.host,
            Field::Port => &mut self.port,
            Field::User => &mut self.user,
            Field::Password => &mut self.password,
            Field::Command => &mut self.command,
            Field::Env => &mut self.env,
            Field::Database => &mut self.database,
            Field::Dsn => &mut self.dsn,
            Field::ServerKey => &mut self.server_key,
            Field::SshHost => &mut self.ssh_host,
            Field::SshPort => &mut self.ssh_port,
            Field::SshUser => &mut self.ssh_user,
            Field::SshKeyFile => &mut self.ssh_key,
            Field::SshSecret => &mut self.ssh_secret,
            Field::SshCommand => &mut self.ssh_command,
            Field::SshEnv => &mut self.ssh_env,
            Field::SshKeepalive => &mut self.ssh_keepalive,
            Field::SshTimeout => &mut self.ssh_timeout,
            _ => return None,
        })
    }

    /// The fields `Tab` visits in the current section, then the buttons: the storage
    /// selector shows only the field of its source.
    pub fn fields(&self) -> Vec<Field> {
        if self.is_tunnel() {
            let mut out = vec![Field::Name];
            out.extend(self.ssh_fields());
            out.extend(BUTTONS);
            return out;
        }
        if self.section == Section::Ssh {
            let mut out = self.ssh_fields();
            out.extend(BUTTONS);
            return out;
        }
        let shown = secret_field(self.source);
        let section: &[Field] = match self.section {
            Section::Basic => &BASIC,
            Section::Advanced | Section::Ssh => &ADVANCED,
        };
        let mysql = self.is_mysql();
        let mut out: Vec<Field> = section
            .iter()
            .copied()
            .filter(|f| !matches!(f, Field::Password | Field::Command | Field::Env) || Some(*f) == shown)
            .filter(|f| !(mysql && matches!(f, Field::SslMode | Field::StatementCache)))
            .filter(|f| mysql || !matches!(f, Field::ServerKey | Field::KeyRetrieval))
            .collect();
        if self.section == Section::Basic {
            out.push(Field::Dsn);
        }
        out.extend(BUTTONS);
        out
    }

    /// The SSH section's fields as the tunnel is set up: the picker alone while there is none
    /// or a preset is picked; the key file only for a key, the secret's source and field only
    /// for a key or a password. A tunnel form has no picker.
    pub fn ssh_fields(&self) -> Vec<Field> {
        if !self.ssh_enabled {
            return vec![Field::SshEnabled];
        }
        if self.is_tunnel() {
            let mut out = self.inline_fields();
            out.retain(|f| *f != Field::SshEnabled);
            return out;
        }
        self.inline_fields()
    }

    /// The SSH section's fields of a tunnel of its own (the picker first).
    fn inline_fields(&self) -> Vec<Field> {
        let secret = self.ssh_auth.has_secret();
        let shown = match self.ssh_source {
            SourceKind::Keychain | SourceKind::File => Some(Field::SshSecret),
            SourceKind::Command => Some(Field::SshCommand),
            SourceKind::Env => Some(Field::SshEnv),
            SourceKind::Prompt => None,
        };
        SSH.iter()
            .copied()
            .filter(|f| match f {
                Field::SshKeyFile => self.ssh_auth == SshAuth::Key,
                Field::SshSource => secret,
                Field::SshSecret | Field::SshCommand | Field::SshEnv => secret && shown == Some(*f),
                _ => true,
            })
            .collect()
    }

    /// The tunnel settings the SSH section describes (typed secrets excluded).
    pub fn ssh_settings(&self) -> SshSettings {
        let secs = |t: &TextInput| t.text().trim().parse::<u64>().ok();
        let mut s = SshSettings {
            enabled: self.ssh_enabled,
            host: self.ssh_host.text().trim().to_string(),
            port: self.ssh_port.text().trim().parse().unwrap_or(0),
            user: self.ssh_user.text().trim().to_string(),
            auth: self.ssh_auth,
            key_file: Some(self.ssh_key.text().trim().to_string()).filter(|k| !k.is_empty()),
            keepalive: secs(&self.ssh_keepalive),
            timeout: secs(&self.ssh_timeout),
            ..SshSettings::default()
        };
        s.set_source(match self.ssh_source {
            SourceKind::Keychain => PasswordSource::Keychain,
            SourceKind::File => PasswordSource::File,
            SourceKind::Command => PasswordSource::Command(self.ssh_command.text().trim().to_string()),
            SourceKind::Env => PasswordSource::Env(self.ssh_env.text().trim().to_string()),
            SourceKind::Prompt => PasswordSource::Prompt,
        });
        s
    }

    /// The passphrase or password typed in the SSH section for a source that stores it, while
    /// the bastion fields are the profile's own tunnel.
    pub fn ssh_typed_secret(&self) -> Option<String> {
        self.typed_bastion_secret().filter(|_| self.ssh_enabled)
    }

    /// The passphrase or password typed for the bastion fields, for a source that stores it
    /// (also once they became a preset this form makes).
    pub fn typed_bastion_secret(&self) -> Option<String> {
        let stores = matches!(self.ssh_source, SourceKind::Keychain | SourceKind::File);
        (self.ssh_auth.has_secret() && stores && !self.ssh_secret.text().is_empty())
            .then(|| self.ssh_secret.text().to_string())
    }

    fn cycle_ssh_auth(&mut self, d: isize) {
        let all = SshAuth::ALL;
        let i = all.iter().position(|a| *a == self.ssh_auth).unwrap_or(0) as isize;
        self.ssh_auth = all[(i + d).rem_euclid(all.len() as isize) as usize];
    }

    fn cycle_ssh_source(&mut self, d: isize) {
        let all = SourceKind::ALL;
        let n = all.len() as isize;
        let mut i = all.iter().position(|k| *k == self.ssh_source).unwrap_or(0) as isize;
        for _ in 0..n {
            i = (i + d).rem_euclid(n);
            if all[i as usize] != SourceKind::Keychain || self.keychain_ok {
                break;
            }
        }
        self.ssh_source = all[i as usize];
    }

    /// Next / previous section (`Ctrl+N` / `Ctrl+P`); the focus goes to its first field. A
    /// tunnel form has one section.
    fn cycle_section(&mut self, d: isize) {
        if self.is_tunnel() {
            return;
        }
        let n = Section::ALL.len() as isize;
        let i = Section::ALL.iter().position(|s| *s == self.section).unwrap_or(0) as isize;
        self.open_section(Section::ALL[(i + d).rem_euclid(n) as usize]);
    }

    /// Show section `s` (a click on its tab); the focus goes to its first field.
    pub fn open_section(&mut self, s: Section) {
        if self.is_tunnel() || s == self.section {
            return;
        }
        self.section = s;
        self.focus = self.fields()[usize::from(self.section == Section::Basic)];
    }

    /// What was drawn at (x, y), the topmost first.
    pub fn hit_at(&self, x: u16, y: u16) -> Option<FormHit> {
        let at = ratatui::layout::Position::new(x, y);
        self.hits.iter().rev().find(|(r, _)| r.contains(at)).map(|(_, h)| *h)
    }

    /// Value `i` of field `f` among those it shows side by side, as the arrows would pick it
    /// (a driver the app has no driver for and the keychain while it does not work are not).
    pub fn pick(&mut self, f: Field, i: usize) {
        let keychain = |k: SourceKind| k != SourceKind::Keychain || self.keychain_ok;
        match f {
            Field::Driver if self.drivers_enabled.get(i) == Some(&true) => self.set_driver(i),
            Field::Source => {
                if let Some(&k) = SourceKind::ALL.get(i).filter(|k| keychain(**k)) {
                    self.source = k;
                }
            }
            Field::SshAuth => {
                if let Some(&a) = SshAuth::ALL.get(i) {
                    self.ssh_auth = a;
                }
            }
            Field::SshSource => {
                if let Some(&k) = SourceKind::ALL.get(i).filter(|k| keychain(**k)) {
                    self.ssh_source = k;
                }
            }
            _ => {}
        }
    }

    /// The choices of a picker field, in order: `None` (automatic / driver / top level) first.
    pub fn choices(&self, f: Field) -> Vec<Option<String>> {
        let mut v: Vec<Option<String>> = vec![None];
        match f {
            Field::Color => {
                v.extend(color::NAMES.iter().map(|n| Some(n.to_string())));
                // A hex color from the file stays a choice.
                if let Some(c) = self.color.as_ref().filter(|c| !color::NAMES.contains(&c.as_str())) {
                    v.push(Some(c.clone()));
                }
            }
            Field::Icon => v.extend(crate::icons::CURATED.iter().map(|c| Some(c.0.to_string()))),
            Field::Folder => v.extend(self.folders.iter().cloned().map(Some)),
            _ => {}
        }
        v
    }

    /// The value of a picker field.
    pub fn choice(&self, f: Field) -> Option<String> {
        match f {
            Field::Color => self.color.clone(),
            Field::Icon => self.icon.clone(),
            Field::Folder => self.folder.clone(),
            _ => None,
        }
    }

    /// Set a picker field.
    pub fn set_choice(&mut self, f: Field, v: Option<String>) {
        match f {
            Field::Color => self.color = v,
            Field::Icon => self.icon = v,
            Field::Folder => self.folder = v,
            _ => {}
        }
    }

    fn cycle_choice(&mut self, f: Field, d: isize) {
        let choices = self.choices(f);
        let cur = self.choice(f);
        let i = choices.iter().position(|c| *c == cur).unwrap_or(0) as isize;
        let n = choices.len() as isize;
        self.set_choice(f, choices[(i + d).rem_euclid(n) as usize].clone());
    }

    /// Next / previous driver that can be picked.
    fn cycle_driver(&mut self, d: isize) {
        let n = DRIVERS.len() as isize;
        let mut i = self.driver as isize;
        for _ in 0..n {
            i = (i + d).rem_euclid(n);
            if self.drivers_enabled[i as usize] {
                break;
            }
        }
        self.set_driver(i as usize);
    }

    /// The color the profile gets with the current choice and name.
    pub fn display_color(&self) -> ProfileColor {
        self.color
            .as_deref()
            .and_then(ProfileColor::parse)
            .unwrap_or_else(|| ProfileColor::auto(self.name.text().trim()))
    }

    /// The source described by the selector and its field.
    pub fn password_source(&self) -> PasswordSource {
        match self.source {
            SourceKind::Keychain => PasswordSource::Keychain,
            SourceKind::File => PasswordSource::File,
            SourceKind::Command => PasswordSource::Command(self.command.text().trim().to_string()),
            SourceKind::Env => PasswordSource::Env(self.env.text().trim().to_string()),
            SourceKind::Prompt => PasswordSource::Prompt,
        }
    }

    /// Next / previous source; the keychain is skipped when it does not work.
    fn cycle_source(&mut self, d: isize) {
        let all = SourceKind::ALL;
        let n = all.len() as isize;
        let mut i = all.iter().position(|k| *k == self.source).unwrap_or(0) as isize;
        for _ in 0..n {
            i = (i + d).rem_euclid(n);
            if all[i as usize] != SourceKind::Keychain || self.keychain_ok {
                break;
            }
        }
        self.source = all[i as usize];
    }

    fn after_edit(&mut self, f: Field) {
        match f {
            Field::Dsn => self.sync_fields(),
            Field::Host | Field::Port | Field::User | Field::Database => self.sync_dsn(),
            Field::SshKeyFile => self.ssh_key_note = None,
            _ => {}
        }
    }

    fn move_focus(&mut self, d: isize) {
        let fields = self.fields();
        let i = fields.iter().position(|f| *f == self.focus).unwrap_or(0) as isize;
        self.focus = fields[(i + d).rem_euclid(fields.len() as isize) as usize];
    }

    fn cycle_sslmode(&mut self, d: isize) {
        self.sslmode = (self.sslmode as isize + d).rem_euclid(SSL_MODES.len() as isize) as usize;
        self.sync_dsn();
    }

    pub fn paste(&mut self, text: &str) {
        let f = self.focus;
        if let Some(inp) = self.input_mut(f) {
            inp.insert_str(text);
            self.after_edit(f);
        }
    }

    pub fn key(&mut self, k: &KeyEvent) -> FormOutcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('s') if ctrl => return FormOutcome::Save,
            KeyCode::Char('t') if ctrl => return FormOutcome::Test,
            KeyCode::Char('n') if ctrl => self.cycle_section(1),
            KeyCode::Char('p') if ctrl => self.cycle_section(-1),
            KeyCode::Esc => return FormOutcome::Cancel,
            KeyCode::Tab | KeyCode::Down => self.move_focus(1),
            KeyCode::BackTab | KeyCode::Up => self.move_focus(-1),
            KeyCode::Enter => match self.focus {
                Field::Test => return FormOutcome::Test,
                Field::Save => return FormOutcome::Save,
                Field::Cancel => return FormOutcome::Cancel,
                f if f.is_picker() => return FormOutcome::Choose(f),
                _ => self.move_focus(1),
            },
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if self.focus.is_picker() => {
                let f = self.focus;
                self.cycle_choice(f, if k.code == KeyCode::Left { -1 } else { 1 })
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if self.focus == Field::Driver => {
                self.cycle_driver(if k.code == KeyCode::Left { -1 } else { 1 })
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if self.focus == Field::SslMode => {
                self.cycle_sslmode(if k.code == KeyCode::Left { -1 } else { 1 })
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if self.focus == Field::StatementCache => {
                self.statement_cache = !self.statement_cache
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if self.focus == Field::KeyRetrieval => {
                self.key_retrieval = !self.key_retrieval
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if self.focus == Field::SshEnabled => {
                self.cycle_ssh_choice(if k.code == KeyCode::Left { -1 } else { 1 })
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if self.focus == Field::SshAuth => {
                self.cycle_ssh_auth(if k.code == KeyCode::Left { -1 } else { 1 })
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if self.focus == Field::SshSource => {
                self.cycle_ssh_source(if k.code == KeyCode::Left { -1 } else { 1 })
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if self.focus == Field::Source => {
                self.cycle_source(if k.code == KeyCode::Left { -1 } else { 1 })
            }
            KeyCode::Left if self.focus.is_button() && self.focus != Field::Test => self.move_focus(-1),
            KeyCode::Right if self.focus.is_button() && self.focus != Field::Cancel => self.move_focus(1),
            _ => {
                let f = self.focus;
                if let Some(inp) = self.input_mut(f)
                    && inp.handle_key(k) == InputResult::Changed
                {
                    self.after_edit(f);
                }
            }
        }
        FormOutcome::None
    }

    /// Field errors. `taken(name)` says whether another profile already uses `name`.
    /// "Required" errors appear only after a save attempt; format errors appear at once.
    pub fn errors(&self, taken: impl Fn(&str) -> bool) -> Vec<(Field, FieldError)> {
        let mut out = Vec::new();
        if self.is_tunnel() {
            // A preset's name is written as typed (blanks around it are an error, not trimmed).
            let name = self.name.text();
            match tunnel::name_problem(name) {
                Some(tunnel::NameProblem::Empty) if self.attempted => out.push((Field::Name, FieldError::Required)),
                Some(tunnel::NameProblem::Empty) => {}
                Some(_) => out.push((Field::Name, FieldError::TunnelName)),
                None if taken(name) => out.push((Field::Name, FieldError::NameTaken)),
                None => {}
            }
            out.extend(self.ssh_errors());
            return out;
        }
        let name = self.name.text().trim();
        if name.is_empty() {
            if self.attempted {
                out.push((Field::Name, FieldError::Required));
            }
        } else if taken(name) {
            out.push((Field::Name, FieldError::NameTaken));
        }
        for (f, inp) in [(Field::Host, &self.host), (Field::User, &self.user)] {
            if self.attempted && inp.text().trim().is_empty() {
                out.push((f, FieldError::Required));
            }
        }
        if self.port.text().trim().is_empty() {
            if self.attempted {
                out.push((Field::Port, FieldError::Required));
            }
        } else if self.port_value().is_none() {
            out.push((Field::Port, FieldError::Port));
        }
        match self.source {
            SourceKind::Command => match command::split(self.command.text()) {
                Err(command::CommandError::Empty) if self.attempted => out.push((Field::Command, FieldError::Required)),
                Err(command::CommandError::Parse) => out.push((Field::Command, FieldError::CommandSyntax)),
                _ => {}
            },
            SourceKind::Env => {
                let name = self.env.text().trim();
                if name.is_empty() {
                    if self.attempted {
                        out.push((Field::Env, FieldError::Required));
                    }
                } else if !valid_env_name(name) {
                    out.push((Field::Env, FieldError::EnvName));
                }
            }
            _ => {}
        }
        // The server key file is read wherever datarig runs: absolute or under `~/` (the
        // driver refuses any other path too).
        let key = self.server_key.text().trim();
        if self.is_mysql() && !key.is_empty() && !key.starts_with("~/") && !std::path::Path::new(key).has_root() {
            out.push((Field::ServerKey, FieldError::AbsolutePath));
        }
        out.extend(self.ssh_errors());
        out
    }

    /// Errors of the SSH section, when the tunnel is on: every field that needs one.
    pub fn ssh_errors(&self) -> Vec<(Field, FieldError)> {
        let mut out = Vec::new();
        if !self.ssh_enabled {
            return out;
        }
        let blank = |t: &TextInput| t.text().trim().is_empty();
        let port = self.ssh_port.text().trim();
        if !port.is_empty() && port.parse::<u16>().ok().filter(|p| *p > 0).is_none() {
            out.push((Field::SshPort, FieldError::Port));
        }
        let secret = self.ssh_auth.has_secret();
        let required = [
            (Field::SshHost, blank(&self.ssh_host)),
            (Field::SshPort, port.is_empty()),
            (Field::SshUser, blank(&self.ssh_user)),
            (Field::SshKeyFile, self.ssh_auth == SshAuth::Key && blank(&self.ssh_key)),
            (Field::SshCommand, secret && self.ssh_source == SourceKind::Command && blank(&self.ssh_command)),
            (Field::SshEnv, secret && self.ssh_source == SourceKind::Env && blank(&self.ssh_env)),
        ];
        if self.attempted {
            out.extend(required.iter().filter(|(_, missing)| *missing).map(|(f, _)| (*f, FieldError::Required)));
        }
        if secret
            && self.ssh_source == SourceKind::Env
            && !blank(&self.ssh_env)
            && !valid_env_name(self.ssh_env.text().trim())
        {
            out.push((Field::SshEnv, FieldError::EnvName));
        }
        for (f, t) in [(Field::SshKeepalive, &self.ssh_keepalive), (Field::SshTimeout, &self.ssh_timeout)] {
            if !blank(t) && t.text().trim().parse::<u64>().is_err() {
                out.push((f, FieldError::Seconds));
            }
        }
        if secret
            && self.ssh_source == SourceKind::Command
            && matches!(command::split(self.ssh_command.text()), Err(command::CommandError::Parse))
        {
            out.push((Field::SshCommand, FieldError::CommandSyntax));
        }
        // Whatever else the settings cannot be used for (the loader's own check).
        if out.is_empty() && self.attempted && self.ssh_settings().problem().is_some() {
            out.push((Field::SshEnabled, FieldError::Required));
        }
        out
    }

    /// The profile described by the fields (password excluded; it goes to its store).
    pub fn to_profile(&self) -> ConnectionConfig {
        let policy = self.policy.text().trim();
        let mut c = ConnectionConfig {
            // The profile's own spelling (`pg`) stays when the driver did not change.
            driver: if driver_index(&self.base.driver) == self.driver {
                self.base.driver.clone()
            } else {
                DRIVERS[self.driver].0.to_string()
            },
            folder: self.folder.clone(),
            color: self.color.clone(),
            icon: self.icon.clone(),
            policy: (!policy.is_empty()).then(|| policy.to_string()),
            name: self.name.text().trim().to_string(),
            host: self.host.text().trim().to_string(),
            port: self.port_value().unwrap_or(self.scheme().default_port()),
            user: self.user.text().to_string(),
            password: String::new(),
            database: self.database.text().to_string(),
            sslmode: SSL_MODES[self.sslmode].to_string(),
            statement_cache: self.statement_cache,
            server_public_key_file: Some(self.server_key.text().trim().to_string()).filter(|k| !k.is_empty()),
            allow_public_key_retrieval: self.key_retrieval,
            // Kept when it existed (also off or with a preset picked); new only once turned on;
            // moved into the preset this form makes, when that one is picked.
            ssh: (!self.new_preset_picked() && (self.ssh_had || self.ssh_enabled)).then(|| self.ssh_settings()),
            tunnel: self.ssh_preset.clone(),
            dsn: None,
            ..self.base.clone()
        };
        c.set_source(self.password_source());
        c
    }

    /// The profile a test connection of the form tries: [`Self::to_profile`], but with the
    /// preset this form makes still the profile's own tunnel (it is not saved yet).
    pub fn to_test_profile(&self) -> ConnectionConfig {
        let mut c = self.to_profile();
        if self.new_preset_picked() {
            c.tunnel = None;
            c.ssh = Some(SshSettings { enabled: true, ..self.ssh_settings() });
        }
        c
    }

    /// The preset "save as tunnel preset" makes when the form is saved, when it is picked.
    pub fn made_preset(&self) -> Option<TunnelPreset> {
        let name = self.new_preset.as_deref().filter(|_| self.new_preset_picked())?;
        Some(TunnelPreset::new(name, SshSettings { enabled: true, ..self.ssh_settings() }))
    }

    /// The folder a new profile goes to (the explorer's folder when it was opened there).
    pub fn set_folder(&mut self, folder: Option<String>) {
        self.folder = folder;
    }

    /// The source the edited profile had when the form opened (`None` for a new profile).
    pub fn original_source(&self) -> Option<SourceKind> {
        self.editing.map(|_| self.base.source().kind())
    }
}

/// Index of a profile's driver in [`DRIVERS`] (`postgresql`/`pg` are `postgres`).
fn driver_index(driver: &str) -> usize {
    let d = match driver.to_ascii_lowercase().as_str() {
        "postgresql" | "pg" => "postgres".to_string(),
        "mariadb" => "mysql".to_string(),
        d => d.to_string(),
    };
    DRIVERS.iter().position(|x| x.0 == d).unwrap_or(0)
}

/// `name`, `name-copy`, `name-copy2`, … whichever is free.
pub fn copy_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = format!("{name}-copy");
    if !taken(&base) {
        return base;
    }
    (2..).map(|i| format!("{base}{i}")).find(|n| !taken(n)).unwrap_or(base)
}

#[cfg(test)]
mod tests;
