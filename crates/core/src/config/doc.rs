//! Every key of the config file, for the generated configuration reference
//! (`docs/site/src/reference/configuration.md`, rendered by the TUI's `tests/config_doc.rs`).
//! That test holds this list to the parser both ways: each documented default, written as TOML,
//! must read back as the value the file has without the key, and each table's keys must be the
//! ones the parser takes (an unknown key names them all). Text here is Markdown: no `|`.

/// A table of the file. `name` is its path with a placeholder for a name the user picks
/// (`policy.<env>`); `""` is the top level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableDoc {
    pub name: &'static str,
    /// How the file writes it (`[[connections]]`).
    pub header: &'static str,
    pub about: &'static str,
    /// The table's old name, still read: the table it is now.
    pub alias_of: Option<&'static str>,
}

/// Whether a key is current, an old name or retired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Key,
    /// An old name of the key named here (same table), read the same; a table with both is an
    /// error.
    Alias(&'static str),
    /// Accepted with any value and ignored; the next save drops it.
    Retired,
}

/// A key of table `table` ([`TableDoc::name`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyDoc {
    pub table: &'static str,
    pub key: &'static str,
    /// What leaving the key out amounts to, written as TOML (`"popup"`, `500`, `[]`).
    /// `None`: no such value; [`Self::unset`] says what then.
    pub default: Option<&'static str>,
    /// Without a [`Self::default`]: what leaving the key out means.
    pub unset: &'static str,
    pub values: &'static str,
    /// Empty for the keys of the TUI's `:set` table: their description is that setting's own.
    pub about: &'static str,
    pub kind: Kind,
}

impl KeyDoc {
    /// Its dotted path (`editor.cursor_shape`, `policy.<env>.read_only`).
    pub fn path(&self) -> String {
        if self.table.is_empty() { self.key.to_string() } else { format!("{}.{}", self.table, self.key) }
    }
}

const fn key(
    table: &'static str,
    key: &'static str,
    default: &'static str,
    values: &'static str,
    about: &'static str,
) -> KeyDoc {
    KeyDoc { table, key, default: Some(default), unset: "", values, about, kind: Kind::Key }
}

const fn unset(
    table: &'static str,
    key: &'static str,
    unset: &'static str,
    values: &'static str,
    about: &'static str,
) -> KeyDoc {
    KeyDoc { table, key, default: None, unset, values, about, kind: Kind::Key }
}

const fn alias(table: &'static str, key: &'static str, of: &'static str) -> KeyDoc {
    KeyDoc { table, key, default: None, unset: "", values: "", about: "", kind: Kind::Alias(of) }
}

/// The tables, in the order of the reference.
pub const TABLES: &[TableDoc] = &[
    TableDoc { name: "", header: "", about: "Keys before the first table header.", alias_of: None },
    TableDoc { name: "editor", header: "[editor]", about: "The query editor.", alias_of: None },
    TableDoc { name: "explorer", header: "[explorer]", about: "The explorer.", alias_of: None },
    TableDoc { name: "ui", header: "[ui]", about: "Things the whole screen shows.", alias_of: None },
    TableDoc { name: "secrets", header: "[secrets]", about: "Passwords and the OS keychain.", alias_of: None },
    TableDoc { name: "commands", header: "[commands]", about: "The `:` command line.", alias_of: None },
    TableDoc {
        name: "connections",
        header: "[[connections]]",
        about: "One table per connection profile, in the explorer's order. Passwords are never \
                written here: `password_source` says where a profile's password comes from.",
        alias_of: None,
    },
    TableDoc {
        name: "connections.ssh",
        header: "[connections.ssh]",
        about: "The SSH tunnel of the profile above it only (a preset several profiles share is a \
                `[tunnels.<name>]` table). One bastion, never read from `~/.ssh/config`. The table \
                stays when the tunnel is turned off.",
        alias_of: None,
    },
    TableDoc {
        name: "policy.<env>",
        header: "[policy.<env>]",
        about: "An environment that profiles name with `policy = \"<env>\"`. Every key is optional; \
                a key left out has its built-in default (not the one of `[policy.default]`). \
                `[policy.default]` is not an environment: it holds the rules of every profile \
                without one (its `color` and `badge` are not shown anywhere). A profile naming an \
                environment that is not defined does not connect.",
        alias_of: None,
    },
    TableDoc {
        name: "tunnels.<name>",
        header: "[tunnels.<name>]",
        about: "An SSH tunnel preset that profiles name with `tunnel = \"<name>\"`: the keys of \
                `[connections.ssh]` without `enabled` (a preset is always on), and an `id`.",
        alias_of: None,
    },
    TableDoc {
        name: "startup.<name>",
        header: "[startup.<name>]",
        about: "A Startup SQL preset: settings a query session of one kind of database starts with. \
                Profiles name it with `startup = \"<name>\"`, environments with `startup = { \
                postgres = \"<name>\", mysql = \"<name>\" }`. The name `none` is reserved.",
        alias_of: None,
    },
    TableDoc {
        name: "sessions.<name>",
        header: "[sessions.<name>]",
        about: "The old name of `[startup.<name>]`, read the same; a file may not have both.",
        alias_of: Some("startup.<name>"),
    },
    TableDoc {
        name: "keymap.<context>",
        header: "[keymap.<context>]",
        about: "Key remapping for one context (see Keybindings for the context names and action \
                ids). Invalid entries are skipped and reported at startup.",
        alias_of: None,
    },
];

/// The keys, by table in [`TABLES`] order.
pub const KEYS: &[KeyDoc] = &[
    // The top level.
    unset(
        "",
        "version",
        "1, the original format",
        "`1`, `2`",
        "The file's format version. datarig writes `2` (every profile has an `id`) and upgrades a \
         version 1 file once at launch; older versions of datarig cannot read version 2.",
    ),
    key("", "language", "\"en\"", "`\"en\"`, `\"ko\"`, `\"auto\"`", ""),
    key("", "icons", "\"auto\"", "`\"auto\"`, `\"on\"`, `\"off\"`", ""),
    key("", "theme", "\"terminal\"", "a built-in theme's name, or the name of a theme file", ""),
    key(
        "",
        "page_size",
        "500",
        "1 to 2147483647",
        "Rows a result fetches from the server at a time (one page of the grid).",
    ),
    key(
        "",
        "result_window_rows",
        "10000",
        "1000 to 10000000",
        "Rows of a result kept in memory; the rest go to a temporary file on disk.",
    ),
    key(
        "",
        "spill_limit",
        "\"1GB\"",
        "bytes as a number, or `\"512MB\"`, `\"2GB\"` (KB, MB and GB are powers of 1024); `0` or \
         `\"off\"` for no limit",
        "The most a result's temporary file may take; fetching stops when it is reached. An \
         environment may set its own.",
    ),
    key(
        "",
        "idle_disconnect",
        "\"off\"",
        "a duration from `\"1m\"` to `\"24h\"` (`\"300s\"`, `\"5m\"`), or `0` or `\"off\"`",
        "",
    ),
    key(
        "",
        "lock_wait",
        "\"off\"",
        "a duration from `\"1s\"` to `\"24h\"` (`\"5s\"`, `\"1m\"`), or `0` or `\"off\"`",
        "",
    ),
    key(
        "",
        "folders",
        "[]",
        "a list of folder paths (`\"work/prod\"`)",
        "Every folder of the explorer, so that empty folders are kept. A profile's own `folder` \
         adds its folder too.",
    ),
    unset(
        "",
        "last_used",
        "none",
        "a profile's `id`",
        "Written by datarig: the profile connected to most recently, where the explorer's cursor \
         starts. A version 1 file has the profile's name here.",
    ),
    key("", "detail_view", "\"panel\"", "`\"panel\"`, `\"statusbar\"`", ""),
    key("", "clipboard", "\"auto\"", "`\"auto\"`, `\"system\"`, `\"osc52\"`", ""),
    key("", "copy_header", "\"auto\"", "`\"auto\"`, `\"on\"`, `\"off\"`", ""),
    key(
        "",
        "osc52_max_bytes",
        "100000",
        "a positive number of bytes",
        "The longest copy sent through OSC 52 (bytes of base64). A longer one is not sent: \
         terminals and tmux drop long OSC 52 sequences without a word.",
    ),
    // [editor]
    KeyDoc {
        table: "editor",
        key: "mode",
        default: None,
        unset: "",
        values: "any",
        about: "Retired (the editor has vim keys only): accepted with any value, ignored, and dropped \
                the next time the app saves the file.",
        kind: Kind::Retired,
    },
    key("editor", "cursor_shape", "\"on\"", "`\"on\"`, `\"off\"`", ""),
    key("editor", "clipboard", "\"on\"", "`\"on\"`, `\"off\"`", ""),
    key("editor", "format_keyword_case", "\"preserve\"", "`\"preserve\"`, `\"upper\"`, `\"lower\"`", ""),
    key("editor", "format_indent", "4", "`4`, `2`", ""),
    key("editor", "auto_pairs", "\"off\"", "`\"off\"`, `\"on\"`", ""),
    key("editor", "soft_tab_backspace", "\"on\"", "`\"on\"`, `\"off\"`", ""),
    key("editor", "run_hints", "\"on\"", "`\"on\"`, `\"off\"`", ""),
    // [explorer]
    key("explorer", "size_heat", "3", "`3`, `5`, `\"off\"` (`\"on\"`, its old value, reads as `3`)", ""),
    // [ui]
    key("ui", "yank_flash", "\"on\"", "`\"on\"`, `\"off\"`", ""),
    key("ui", "tips", "\"on\"", "`\"on\"`, `\"off\"`", ""),
    // [secrets]
    key(
        "secrets",
        "default_source",
        "\"auto\"",
        "`\"auto\"`, `\"keychain\"`, `\"file\"`, `\"command\"`, `\"env\"`, `\"prompt\"`",
        "",
    ),
    key("secrets", "macos_keychain_access", "\"direct\"", "`\"direct\"`, `\"security_tool\"`", ""),
    // [commands]
    key("commands", "position", "\"popup\"", "`\"popup\"`, `\"bottom\"`", ""),
    // [[connections]]
    unset(
        "connections",
        "id",
        "a new one, written by the launch-time upgrade",
        "a UUID",
        "The profile's stable id: its keychain entries and `last_used` refer to it, so a rename \
         keeps them. datarig assigns it; two profiles with one id are an error.",
    ),
    key("connections", "name", "\"local-pg\"", "text", "The name the explorer shows and `datarig <profile>` takes."),
    key(
        "connections",
        "driver",
        "\"postgres\"",
        "`\"postgres\"` (also `\"postgresql\"`, `\"pg\"`), `\"mysql\"` (also `\"mariadb\"`)",
        "The kind of database.",
    ),
    key("connections", "host", "\"127.0.0.1\"", "a host name or address", "The server's host."),
    key(
        "connections",
        "port",
        "5432",
        "a port number",
        "The server's port. Left out it is 5432 whatever the driver, so a MySQL profile says `3306`.",
    ),
    key("connections", "user", "\"\"", "text", "The database user."),
    unset(
        "connections",
        "password",
        "none",
        "text",
        "A plaintext password from an older file. At launch it is moved to the profile's password \
         store (the keychain or the secrets file) and removed from the file; datarig never writes it.",
    ),
    key("connections", "database", "\"\"", "text", "The database to connect to."),
    unset(
        "connections",
        "sslmode",
        "PostgreSQL `\"disable\"`; MySQL `\"PREFERRED\"` (`\"DISABLED\"` with `server_public_key_file`)",
        "PostgreSQL: `\"disable\"`, `\"prefer\"`, `\"require\"`, `\"verify-ca\"`, `\"verify-full\"`; \
         MySQL: `\"DISABLED\"`, `\"PREFERRED\"`, `\"REQUIRED\"`, `\"VERIFY_CA\"`, `\"VERIFY_IDENTITY\"`",
        "How the connection uses TLS, spelled as the database's own client spells it.",
    ),
    unset(
        "connections",
        "sslrootcert",
        "none",
        "a file path",
        "The CA file the server's certificate chain is checked against.",
    ),
    unset("connections", "sslcert", "none", "a file path", "The client certificate file, used with `sslkey`."),
    unset("connections", "sslkey", "none", "a file path", "The client certificate's private key file."),
    unset(
        "connections",
        "dsn",
        "none",
        "a connection string",
        "A connection string kept as written because it cannot be turned into fields (not a URL, or \
         parameters other than `sslmode`, `sslrootcert`, `sslcert` and `sslkey`). A URL of the \
         profile's driver with only those becomes fields when the file is read.",
    ),
    unset(
        "connections",
        "folder",
        "the top level",
        "a folder path (`\"work/prod\"`)",
        "The explorer folder the profile is in. Folders only organize profiles.",
    ),
    unset(
        "connections",
        "color",
        "a color picked from the name",
        "`\"red\"`, `\"orange\"`, `\"yellow\"`, `\"green\"`, `\"teal\"`, `\"cyan\"`, `\"blue\"`, \
         `\"indigo\"`, `\"purple\"`, `\"pink\"`, `\"brown\"`, `\"gray\"`, or `\"#rrggbb\"`",
        "The profile's color in the explorer and on its tabs.",
    ),
    unset(
        "connections",
        "icon",
        "the driver's icon",
        "the name of one of datarig's icons",
        "The Nerd Font icon next to the profile (an unknown name shows the driver's icon).",
    ),
    unset(
        "connections",
        "policy",
        "the default rules",
        "the name of a `[policy.<env>]` table",
        "The profile's environment. A name no environment has is an error of the profile: it does \
         not connect.",
    ),
    key(
        "connections",
        "password_source",
        "\"keychain\"",
        "`\"keychain\"`, `\"file\"` (the secrets file), `\"command\"`, `\"env\"`, `\"prompt\"`",
        "Where the profile's password comes from.",
    ),
    unset(
        "connections",
        "password_command",
        "none",
        "a command",
        "With `password_source = \"command\"` (required then): the command whose output is the password.",
    ),
    unset(
        "connections",
        "password_env",
        "none",
        "an environment variable's name",
        "With `password_source = \"env\"` (required then): the variable that holds the password.",
    ),
    key(
        "connections",
        "statement_cache",
        "true",
        "`true`, `false`",
        "Keep prepared statements on the server between transactions. Turn it off for a server \
         behind a pooler in transaction mode that does not support them (PgBouncer before 1.22, or \
         with `max_prepared_statements = 0`); the driver also turns it off for a connection when \
         the server keeps losing them.",
    ),
    unset(
        "connections",
        "server_public_key_file",
        "none",
        "a PEM file path",
        "MySQL: the server's RSA public key. A `caching_sha2_password` login over an unencrypted \
         connection encrypts the password with it and never asks the server for a key.",
    ),
    key(
        "connections",
        "allow_public_key_retrieval",
        "false",
        "`true`, `false`",
        "MySQL: whether a direct, unencrypted connection to a server that is not on this machine may \
         ask the server for its public key. Whoever sits in between could send theirs and read the \
         password, so such a login is refused when off. A loopback server and one reached through an \
         SSH tunnel are asked either way.",
    ),
    unset(
        "connections",
        "tunnel",
        "a direct connection",
        "the name of a `[tunnels.<name>]` table",
        "The tunnel preset the profile connects through. A preset that is not there, or a preset \
         and an enabled `[connections.ssh]` at once, is an error of the profile.",
    ),
    unset(
        "connections",
        "startup",
        "the preset its environment maps for its database, if any",
        "the name of a `[startup.<name>]` table, or `\"none\"`",
        "The Startup SQL preset its query sessions start with; `\"none\"`: none, whatever its \
         environment maps. A preset that is not there, or one for the other database, is an error \
         of the profile.",
    ),
    key(
        "connections",
        "startup_sql",
        "[]",
        "a list of statements",
        "The profile's own session settings, run after the preset's. Each must be one setting of \
         the session; they are checked when the profile connects.",
    ),
    alias("connections", "session", "startup"),
    alias("connections", "session_sql", "startup_sql"),
    // [connections.ssh]
    key("connections.ssh", "enabled", "false", "`true`, `false`", "Whether the profile connects through this tunnel."),
    unset("connections.ssh", "host", "none (required when enabled)", "a host name or address", "The bastion's host."),
    key("connections.ssh", "port", "22", "a port number (`0` is refused when enabled)", "The bastion's SSH port."),
    unset("connections.ssh", "user", "none (required when enabled)", "text", "The user on the bastion."),
    key(
        "connections.ssh",
        "auth",
        "\"key\"",
        "`\"key\"`, `\"password\"`, `\"agent\"`, `\"keyboard-interactive\"`",
        "How the tunnel logs in: a private key file (OpenSSH or PEM, maybe with a passphrase), a \
         password, the keys of the ssh-agent, or the bastion's own questions (a one-time code).",
    ),
    unset(
        "connections.ssh",
        "key_file",
        "none (required with `auth = \"key\"`)",
        "a file path (`~` is the home directory)",
        "The private key file.",
    ),
    key(
        "connections.ssh",
        "secret_source",
        "\"keychain\"",
        "as `password_source`",
        "Where the key's passphrase or the password comes from. It is kept apart from the database \
         password.",
    ),
    unset(
        "connections.ssh",
        "secret_command",
        "none",
        "a command",
        "With `secret_source = \"command\"` (required then).",
    ),
    unset(
        "connections.ssh",
        "secret_env",
        "none",
        "an environment variable's name",
        "With `secret_source = \"env\"` (required then).",
    ),
    key("connections.ssh", "keepalive", "15", "seconds; `0` for none", "Seconds between keepalives."),
    key(
        "connections.ssh",
        "timeout",
        "10",
        "seconds (`0` counts as 1)",
        "Seconds each step of opening the tunnel may take.",
    ),
    // [policy.<env>]
    key(
        "policy.<env>",
        "paging",
        "\"no_hold\"",
        "`\"no_hold\"`, `\"hold\"`",
        "What a result with more rows than a page keeps on the server, outside your own `BEGIN` \
         block. `\"no_hold\"` keeps nothing: the next page runs the statement again (only a plain \
         `SELECT`; any other statement shows its first page only). `\"hold\"` keeps the cursor, \
         its transaction and its locks open until the result is read to its end or closed after \
         `paging_idle_timeout`.",
    ),
    key(
        "policy.<env>",
        "paging_idle_timeout",
        "\"30s\"",
        "seconds as a number, or `\"90s\"`, `\"5m\"`, `\"1h\"`; `0` or `\"off\"` never closes it",
        "With `paging = \"hold\"`: how long a result that has more rows may keep its cursor open \
         without a fetch.",
    ),
    unset(
        "policy.<env>",
        "spill_limit",
        "the top-level `spill_limit`",
        "as the top-level `spill_limit`",
        "The most a result's temporary file may take for its profiles.",
    ),
    key(
        "policy.<env>",
        "read_only",
        "false",
        "`true`, `false`",
        "Run only reads, transaction control and safe session settings, and refuse the rest before \
         sending anything; the server also makes every transaction of the query sessions read-only.",
    ),
    key(
        "policy.<env>",
        "confirm",
        "\"destructive\"",
        "`\"destructive\"`, `\"writes\"`",
        "Which statements ask before they run: destructive ones (`DROP`, `TRUNCATE`, an `UPDATE` or \
         `DELETE` of every row, …), or also every other statement that may write. Dangerous \
         statements always ask.",
    ),
    key(
        "policy.<env>",
        "commit",
        "\"auto\"",
        "`\"auto\"`, `\"manual\"`",
        "`\"auto\"`: each statement outside your own `BEGIN` block commits by itself. `\"manual\"`: \
         the first statement that may write opens a transaction that stays open until you commit or \
         roll it back. A driver that cannot runs it as `\"auto\"`.",
    ),
    key(
        "policy.<env>",
        "tx_idle_warning",
        "\"5m\"",
        "a duration as `paging_idle_timeout` takes, at least 10 seconds, or `0` or `\"off\"`",
        "How long an open transaction may sit idle before the app warns that it holds locks.",
    ),
    key(
        "policy.<env>",
        "tx_idle_rollback",
        "\"30m\"",
        "a duration as `paging_idle_timeout` takes, at least 10 seconds, or `0` or `\"off\"`",
        "How long a transaction opened for manual commit may sit idle before the app rolls it back. \
         PostgreSQL's server also gets the limit, a little later, so it ends there when the app is \
         gone.",
    ),
    alias("policy.<env>", "tx_idle_timeout", "tx_idle_rollback"),
    unset(
        "policy.<env>",
        "idle_disconnect",
        "the top-level `idle_disconnect`",
        "as the top-level `idle_disconnect`",
        "How long its profiles' connections may sit unused before the app closes their sessions.",
    ),
    unset(
        "policy.<env>",
        "lock_wait",
        "the top-level `lock_wait`",
        "as the top-level `lock_wait`",
        "How long a statement of its profiles' query sessions waits for another session's lock. A \
         Startup SQL statement that sets the server's own limit overrides it for those sessions.",
    ),
    unset(
        "policy.<env>",
        "startup",
        "none",
        "`{ postgres = \"<name>\", mysql = \"<name>\" }`, either left out",
        "The Startup SQL preset its profiles' query sessions start with, for each database. A \
         profile may name its own, or `\"none\"`.",
    ),
    alias("policy.<env>", "session", "startup"),
    unset(
        "policy.<env>",
        "color",
        "none",
        "as a profile's `color`",
        "The environment's color: its profiles' names take it in the explorer, the tab list and the \
         quick switcher, and its tabs' titles in the tab bar.",
    ),
    unset(
        "policy.<env>",
        "badge",
        "none",
        "text of at most 8 columns, no blanks around it",
        "A short text its profiles' tabs and the status bar show (`\"PROD\"`).",
    ),
    // [tunnels.<name>]
    unset(
        "tunnels.<name>",
        "id",
        "a new one, written by datarig",
        "a UUID",
        "The preset's stable id: its secret is kept under it, so a rename keeps it.",
    ),
    unset("tunnels.<name>", "host", "none (required)", "a host name or address", "The bastion's host."),
    key("tunnels.<name>", "port", "22", "1 to 65535", "The bastion's SSH port."),
    unset("tunnels.<name>", "user", "none (required)", "text", "The user on the bastion."),
    key(
        "tunnels.<name>",
        "auth",
        "\"key\"",
        "`\"key\"`, `\"password\"`, `\"agent\"`, `\"keyboard-interactive\"`",
        "As in `[connections.ssh]`.",
    ),
    unset(
        "tunnels.<name>",
        "key_file",
        "none (required with `auth = \"key\"`)",
        "a file path (`~` is the home directory)",
        "The private key file.",
    ),
    key(
        "tunnels.<name>",
        "secret_source",
        "\"keychain\"",
        "as `password_source`",
        "Where the key's passphrase or the password comes from.",
    ),
    unset(
        "tunnels.<name>",
        "secret_command",
        "none",
        "a command",
        "With `secret_source = \"command\"` (required then).",
    ),
    unset(
        "tunnels.<name>",
        "secret_env",
        "none",
        "an environment variable's name",
        "With `secret_source = \"env\"` (required then).",
    ),
    key("tunnels.<name>", "keepalive", "15", "seconds; `0` for none", "Seconds between keepalives."),
    key(
        "tunnels.<name>",
        "timeout",
        "10",
        "seconds (`0` counts as 1)",
        "Seconds each step of opening the tunnel may take.",
    ),
    // [startup.<name>]
    unset(
        "startup.<name>",
        "dbms",
        "none (required)",
        "`\"postgres\"`, `\"mysql\"`",
        "The kind of database the preset is for.",
    ),
    key(
        "startup.<name>",
        "sql",
        "[]",
        "a list of statements",
        "The statements a query session starts with, in order. Each must be one setting of the \
         session (in a read-only environment one a read-only session may change); they are checked \
         when a profile connects.",
    ),
    // [keymap.<context>]
    unset(
        "keymap.<context>",
        "\"<keys>\"",
        "the default bindings",
        "an action id, or `\"none\"` to unbind",
        "Binds the keys (`\"ctrl+e\"`, `\"space c n\"`: a space separates the keys of a sequence) \
         in that context.",
    ),
];

/// The table `name` belongs to: `name` itself, or the one with a placeholder where the user
/// picks a name (`policy.prod` is `policy.<env>`, `keymap.editor.vim.normal` is
/// `keymap.<context>`). `None` for a table the file does not have.
pub fn table(name: &str) -> Option<&'static TableDoc> {
    TABLES.iter().find(|t| t.name == name).or_else(|| {
        let (head, rest) = name.split_once('.')?;
        let named = |t: &&TableDoc| t.name.split_once('.').is_some_and(|(h, n)| h == head && n.starts_with('<'));
        if rest.is_empty() { None } else { TABLES.iter().find(named) }
    })
}

/// Key `key` of the table `name` ([`table`]).
pub fn find(name: &str, key: &str) -> Option<&'static KeyDoc> {
    let t = table(name)?;
    KEYS.iter().find(|k| k.table == t.name && k.key == key)
}
