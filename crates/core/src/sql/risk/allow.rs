//! What a read-only policy may still let run of the calls the classifier could not check
//! ([`Risk::unchecked_calls`](super::Risk::unchecked_calls)): the built-ins a newer PostgreSQL
//! adds, when the server is that new, and the functions an environment allows
//! (`read_only_functions`).
//!
//! The classifier itself knows PostgreSQL 17's built-ins only ([`super::BUILTINS`]), so its
//! answer does not depend on the server and can be kept. The server's major version and the
//! environment's list are applied when a read-only policy decides
//! ([`super::Risk::read_only_in`]). Without them ([`ReadOnlyContext::default`]) nothing more
//! runs than PostgreSQL 17's built-ins: an unknown or older server is never given more.

/// The names of the functions PostgreSQL 18 adds to its catalog (the `pg_proc` rows of
/// `pg_catalog` that initdb creates, `oid < 10000`) that PostgreSQL 17 does not have, one per
/// line in byte order: the server's own list less [`super::BUILTINS`] (and less the names of
/// [`super::RUNS_CODE`], none of which are new). Those that act on the server are in
/// [`super::SERVER_ACTIONS`] and [`super::SERVER_FILES`] too, which a read-only policy refuses
/// whatever the version.
pub const BUILTINS_18: &str = include_str!("builtins_18.txt");

/// The PostgreSQL 17 built-ins PostgreSQL 18 no longer has (`pg_timezone_abbrevs` became
/// `pg_timezone_abbrevs_zone` and `pg_timezone_abbrevs_abbrevs`): on PostgreSQL 18 or later a
/// call of one is a call of a function that is not a built-in.
pub const REMOVED_IN_18: &[&str] = &["pg_timezone_abbrevs"];

/// Whether `name` (as PostgreSQL folds it) is a function PostgreSQL 18 adds ([`BUILTINS_18`]).
pub fn builtin_18(name: &str) -> bool {
    static SORTED: std::sync::OnceLock<Vec<&str>> = std::sync::OnceLock::new();
    SORTED.get_or_init(|| BUILTINS_18.lines().collect()).binary_search(&name).is_ok()
}

/// What a read-only policy may let run beyond PostgreSQL 17's built-ins
/// ([`super::Risk::read_only_in`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadOnlyContext {
    /// The PostgreSQL server's major version (`server_version_num / 10000`), when the session
    /// read it. `None` (not known, or not PostgreSQL) lets only PostgreSQL 17's built-ins run.
    pub server_major: Option<u32>,
    /// The functions the environment allows on read-only (`read_only_functions`).
    pub allowed: Vec<AllowedFunction>,
    /// The functions of the extensions the environment allows on read-only
    /// (`read_only_extensions`), as the database's catalog lists them.
    pub extensions: ExtensionLookup,
    /// The extensions the environment allows (`read_only_extensions`), as written.
    pub wanted_extensions: Vec<String>,
}

/// What the database's catalog says of the extensions an environment allows on read-only
/// (`read_only_extensions`): read by the app's metadata session for the tab's database and
/// connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ExtensionLookup {
    /// The environment allows no extension.
    #[default]
    None,
    /// Not read yet for this database and connection.
    Pending,
    /// The read failed: nothing of the extensions runs.
    Failed,
    /// The catalog's answer.
    Read(ExtensionFunctions),
}

/// The functions of the allowed extensions installed in a database, and every other function
/// that has the name of one of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtensionFunctions {
    /// The allowed extensions installed in the database.
    pub installed: Vec<String>,
    /// The allowed extensions' functions, and the functions of the same names outside them.
    pub functions: Vec<CatalogFunction>,
}

/// A function of the catalog ([`ExtensionFunctions`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogFunction {
    /// Its schema.
    pub schema: String,
    /// Its name.
    pub name: String,
    /// The allowed extension it belongs to; `None` for a function that only shares a name with
    /// one of theirs.
    pub extension: Option<String>,
}

/// Why a call a read-only policy refuses was not let run ([`ReadOnlyContext::hint`]): what the
/// refusal adds to its reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallHint {
    /// Nothing allows it: it could be added to `read_only_functions`.
    NotAllowed,
    /// The allowed extensions' functions are not read yet.
    ExtensionsPending,
    /// Reading the allowed extensions' functions failed.
    ExtensionsFailed,
    /// Allowed extensions that the database does not have, named.
    NotInstalled(Vec<String>),
    /// A call of an allowed extension's function's name that another function shares (in the
    /// call's schema, or anywhere for an unqualified call), or in a schema the extension's
    /// function is not in: which one runs is not certain.
    SharedName,
}

impl ReadOnlyContext {
    /// Whether a call by `name` (its names as the parse tree gives them: `["f"]`, `["s", "f"]`,
    /// `["db", "s", "f"]`) may run: a built-in of the server's version or an allowed function.
    pub fn lets_run(&self, name: &[String]) -> bool {
        let newer = self.server_major.is_some_and(|v| v >= 18)
            && matches!(name, [f] | [_, f] | [_, _, f] if builtin_18(f) && catalog(name));
        newer || self.allows(name)
    }

    /// Whether the environment allows a call by `name`: in `read_only_functions`, or a
    /// function of an extension in `read_only_extensions`.
    pub fn allows(&self, name: &[String]) -> bool {
        self.allowed.iter().any(|a| a.matches(name)) || self.extension_function(name)
    }

    /// Whether a call by `name` is an allowed extension's function, whichever of the
    /// functions of that name the server picks: one of them belongs to an allowed extension,
    /// and no function outside the allowed extensions has that name, in that schema when the
    /// call names one (also with a database), in any schema when it does not (the search path
    /// picks then; unqualified calls never find functions of `pg_temp`). Overloads count: a
    /// function of the user's with the extension function's name and other arguments in the
    /// same schema may be the one the arguments pick.
    fn extension_function(&self, name: &[String]) -> bool {
        let ExtensionLookup::Read(read) = &self.extensions else { return false };
        let (schema, f) = match name {
            [f] => (None, f),
            [s, f] | [_, s, f] => (Some(s), f),
            _ => return false,
        };
        let named = |c: &&CatalogFunction| &c.name == f && schema.is_none_or(|s| &c.schema == s);
        let mut same = read.functions.iter().filter(named).peekable();
        same.peek().is_some() && same.all(|c| c.extension.is_some())
    }

    /// What the refusal of a call by `name` should add to its reason.
    pub fn hint(&self, name: &[String]) -> CallHint {
        match &self.extensions {
            ExtensionLookup::None => CallHint::NotAllowed,
            ExtensionLookup::Pending => CallHint::ExtensionsPending,
            ExtensionLookup::Failed => CallHint::ExtensionsFailed,
            ExtensionLookup::Read(read) => {
                let f = name.last();
                if read.functions.iter().any(|c| c.extension.is_some() && Some(&c.name) == f) {
                    return CallHint::SharedName;
                }
                let wanted = self.wanted_extensions.iter();
                let missing: Vec<String> = wanted.filter(|e| !read.installed.contains(e)).cloned().collect();
                if missing.is_empty() { CallHint::NotAllowed } else { CallHint::NotInstalled(missing) }
            }
        }
    }
}

/// Whether `name` is unqualified or in `pg_catalog` (also with a database).
fn catalog(name: &[String]) -> bool {
    match name {
        [_] => true,
        [s, _] | [_, s, _] => s == "pg_catalog",
        _ => false,
    }
}

/// A function an environment lets a read-only policy call (`read_only_functions`): its name,
/// folded as PostgreSQL folds it, with the schema when the entry gives one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowedFunction {
    /// The schema, when the entry names one (`public.crypt`).
    pub schema: Option<String>,
    /// The function's name.
    pub name: String,
    /// The entry as written in the config (shown, and written back unchanged).
    pub entry: String,
}

impl AllowedFunction {
    /// The entry `entry`, written as in a statement: `st_x`, `public.crypt`, `"MyFn"`,
    /// `app."Fn"` (folded to lower case unless quoted, read by PostgreSQL's own parser). `None`
    /// when it is not one or two names: empty, three names, arguments, anything else.
    pub fn parse(entry: &str) -> Option<Self> {
        if entry.trim().is_empty() || entry.contains(['(', ')', ';']) || entry.contains("--") || entry.contains("/*") {
            return None;
        }
        let stmts = super::parse(&format!("SELECT {entry}()"))?;
        let [stmt] = stmts.as_slice() else { return None };
        let (_, select) = super::node(stmt).filter(|(k, _)| *k == "SelectStmt")?;
        let [target] = select["target_list"].as_array()?.as_slice() else { return None };
        let (_, target) = super::node(target).filter(|(k, _)| *k == "ResTarget")?;
        if target["name"].as_str().is_some_and(|n| !n.is_empty()) {
            return None;
        }
        let (_, call) = super::node(&target["val"]).filter(|(k, _)| *k == "FuncCall")?;
        let names: Vec<&str> = call["funcname"].as_array()?.iter().filter_map(super::text).collect();
        match names.as_slice() {
            [f] => Some(Self { schema: None, name: f.to_string(), entry: entry.to_string() }),
            [s, f] => Some(Self { schema: Some(s.to_string()), name: f.to_string(), entry: entry.to_string() }),
            _ => None,
        }
    }

    /// Whether a call by `name` is this function: one without a schema matches the name
    /// however it is qualified; one with a schema only a call qualified with that schema (also
    /// with a database), never an unqualified one, which the search path resolves.
    pub fn matches(&self, name: &[String]) -> bool {
        let Some((f, qualifiers)) = name.split_last() else { return false };
        if *f != self.name {
            return false;
        }
        match &self.schema {
            None => true,
            Some(s) => matches!(qualifiers, [q] | [_, q] if q == s),
        }
    }
}
