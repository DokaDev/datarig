//! Error texts for driver, TLS and dial failures.

use crate::app::persist;
use crate::app::*;
use datarig_core::driver::{CancelFailure, DbError};
use datarig_core::fault::{ErrorLog, FaultKind};
use datarig_core::tls::{FileProblem, TlsFile, TlsProblem};
use datarig_core::transport::{DialError, Refusal};

impl App {
    /// The text of the failure an event carries (empty for other events): see
    /// [`App::db_error_text`].
    pub(in crate::app) fn event_error_text(&self, ev: &DbEvent) -> String {
        match ev {
            DbEvent::ConnectFailed { error, .. } | DbEvent::Failed { error, .. } | DbEvent::Lost { error } => {
                self.db_error_text(error)
            }
            // The version the key metadata needs is the structure's too, in its own words.
            DbEvent::Structure { result: Err(DbError::ServerTooOld), .. } => {
                self.i18n.label(Label::TreeStructureTooOld).to_string()
            }
            DbEvent::Structure { result: Err(DbError::Locked), .. } => {
                self.i18n.label(Label::TreeStructureLocked).to_string()
            }
            DbEvent::Schemas(Err(error))
            | DbEvent::Objects { result: Err(error), .. }
            | DbEvent::Structure { result: Err(error), .. }
            | DbEvent::Catalog(Err(error))
            | DbEvent::Keys(Err(error)) => self.db_error_text(error),
            _ => String::new(),
        }
    }

    /// Why an `EXPLAIN ANALYZE` was not sent, naming its tables (`schema.table (engine)`) in the
    /// UI's language; with tables whose triggers the account cannot see, what to do about it.
    fn not_undoable_text(&self, tables: &[String]) -> String {
        const TRIGGER: &str = " (trigger)";
        const HIDDEN: &str = " (triggers not visible)";
        let hidden = tables.iter().any(|t| t.ends_with(HIDDEN));
        let named: Vec<String> = tables
            .iter()
            .map(|t| {
                if let Some(name) = t.strip_suffix(HIDDEN) {
                    format!("{name} ({})", self.i18n.label(Label::DbNotUndoableTriggersHidden))
                } else if let Some(name) = t.strip_suffix(TRIGGER) {
                    format!("{name} ({})", self.i18n.label(Label::DbNotUndoableTrigger))
                } else {
                    t.clone()
                }
            })
            .collect();
        let tables = named.join(", ");
        let m = if hidden { Msg::DbNotUndoableNoTrigger { tables } } else { Msg::DbNotUndoable { tables } };
        self.i18n.msg(&m).to_string()
    }

    /// A database failure as the UI says it: the server's own message as it is (it is data),
    /// anything else in words, with its raw detail in the error log.
    pub(in crate::app) fn db_error_text(&self, e: &DbError) -> String {
        let label = match e {
            DbError::Server(m) => return m.text.clone(),
            DbError::NeedsNoTransaction(s)
            | DbError::IdleInTransactionTimeout(s)
            | DbError::IdleSessionTimeout(s)
            | DbError::LockBusy(s) => {
                return s.clone();
            }
            DbError::LockTimeout(s) | DbError::ServerLockTimeout(s) => {
                return self.i18n.msg(&Msg::DbLockTimeoutAny { error: s.clone() }).to_string();
            }
            DbError::NoAnswer(d) => return self.i18n.msg(&Msg::DbNoAnswer { elapsed: *d }).to_string(),
            DbError::TimedOut(d) => {
                return self
                    .i18n
                    .msg(&Msg::DbStoppedAtLimit { limit: crate::app::estimate::fmt_limit(*d) })
                    .to_string();
            }
            DbError::RowsNotRead(n) => return self.i18n.msg(&Msg::DbRowsNotRead { count: *n }).to_string(),
            DbError::NotUndoable(t) if t.is_empty() => Label::DbNotUndoableUnread,
            DbError::NotUndoable(t) => return self.not_undoable_text(t),
            DbError::Closed => Label::DbClosed,
            DbError::NotSupported => Label::DbNotSupported,
            DbError::ServerTooOld => Label::DbServerTooOld,
            DbError::SchemaChanged => Label::DbSchemaChanged,
            DbError::NoResult => Label::DbNoResult,
            DbError::Cancelled => Label::QueryCancelled,
            DbError::ReadWriteRefused => Label::DbReadWriteRefused,
            DbError::CopyStdio => Label::DbCopyStdio,
            DbError::CappedOutcome => Label::DbCappedOutcome,
            DbError::CopyStarted => Label::DbCopyStarted,
            DbError::ReadOnlyLost => Label::DbReadOnlyLost,
            DbError::NoTransaction => Label::DbNoTransaction,
            DbError::TxNotOpened => Label::DbTxNotOpened,
            DbError::CommitUnknown => Label::DbCommitUnknown,
            DbError::ApplyNotUndone => Label::DbApplyNotUndone,
            DbError::ReservedSavepoint => Label::DbReservedSavepoint,
            DbError::NotAPlainRead => Label::DbNotAPlainRead,
            DbError::NextTransactionSet => Label::DbNextTransactionSet,
            DbError::SessionSql { index, statement, error } => {
                let m = Msg::DbSessionSql {
                    index: (index + 1).to_string(),
                    error: self.db_error_text(error),
                    sql: crate::app::runlog::excerpt(statement, 60),
                };
                return self.i18n.msg(&m).to_string();
            }
            DbError::CancelNotTaken(CancelFailure::Refused) => Label::DbCancelRefused,
            // Why it could not be sent, in its own words (logged as they log it).
            DbError::CancelNotTaken(CancelFailure::NotSent(why)) => {
                return self.i18n.msg(&Msg::DbCancelNotSent { reason: self.db_error_text(why) }).to_string();
            }
            DbError::CancelNotTaken(CancelFailure::KeptSending) => Label::DbCancelKeptSending,
            DbError::CancelNotTaken(CancelFailure::NotAnswered(d)) => {
                return self.i18n.msg(&Msg::DbCancelNotAnswered { elapsed: *d }).to_string();
            }
            DbError::CancelNotTaken(CancelFailure::StopPending(d)) => {
                return self.i18n.msg(&Msg::DbCancelStopPending { elapsed: *d }).to_string();
            }
            DbError::Locked => Label::DbLocked,
            DbError::NotFound => Label::DbNotFound,
            DbError::Ambiguous(kinds) => {
                use datarig_core::driver::ddl::ObjectKind as K;
                // The example names a kind the name has: the last, a routine or such rather than
                // the table the explorer shows already.
                let kind = match kinds.last() {
                    Some(K::Table) | None => "table",
                    Some(K::View) => "view",
                    Some(K::Procedure) => "procedure",
                    Some(K::Function) => "function",
                    Some(K::Trigger) => "trigger",
                    Some(K::Event) => "event",
                }
                .to_string();
                let kinds: Vec<String> = kinds
                    .iter()
                    .map(|k| {
                        self.i18n
                            .label(match k {
                                K::Table => Label::DbKindTable,
                                K::View => Label::DbKindView,
                                K::Procedure => Label::DbKindProcedure,
                                K::Function => Label::DbKindFunction,
                                K::Trigger => Label::DbKindTrigger,
                                K::Event => Label::DbKindEvent,
                            })
                            .to_string()
                    })
                    .collect();
                return self.i18n.msg(&Msg::DbAmbiguous { kinds: kinds.join(", "), kind }).to_string();
            }
            DbError::DefinitionHidden => Label::DbDefinitionHidden,
            DbError::NotAName => Label::DbNotAName,
            DbError::NoDatabase => Label::DbNoDatabase,
            DbError::KindUnsupported(kind) => {
                return self.i18n.msg(&Msg::DbKindUnsupported { kind: kind.clone() }).to_string();
            }
            DbError::TlsRequired => Label::DbTlsRequired,
            DbError::Tls(p) => return self.tls_problem_text(p),
            DbError::VersionUnsupported { server, needed } => {
                return self
                    .i18n
                    .msg(&Msg::DbVersionUnsupported { server: server.clone(), needed: needed.clone() })
                    .to_string();
            }
            DbError::AuthUnsupported(name) => {
                return self.i18n.msg(&Msg::DbAuthUnsupported { name: name.clone() }).to_string();
            }
            DbError::AccessDenied(message) => {
                return self.i18n.msg(&Msg::DbAccessDenied { message: message.clone() }).to_string();
            }
            DbError::KeyRetrievalRefused => Label::DbKeyRetrievalRefused,
            DbError::ServerKeyInvalid => Label::DbServerKeySentInvalid,
            DbError::ServerKeyFile { path, fault: None } => {
                return self.i18n.msg(&Msg::DbServerKeyInvalid { path: path.clone() }).to_string();
            }
            DbError::ServerKeyPathRelative(path) => {
                return self.i18n.msg(&Msg::DbServerKeyRelative { path: path.clone() }).to_string();
            }
            DbError::PasswordTooLong { max } => {
                return self.i18n.msg(&Msg::DbPasswordTooLong { max: max.to_string() }).to_string();
            }
            DbError::ServerKeyFile { path, fault: Some(f) } => {
                ErrorLog::new(self.paths.errors_log()).record("db.server_key", f);
                let error = self.i18n.msg(&persist::fault_reason(f)).to_string();
                return self.i18n.msg(&Msg::DbServerKeyUnreadable { path: path.clone(), error }).to_string();
            }
            DbError::NotRepeatable(r) => return self.i18n.msg(&crate::app::pages::why(r)).to_string(),
            DbError::Settings(f) => {
                ErrorLog::new(self.paths.errors_log()).record("db.settings", f);
                Label::DbSettings
            }
            DbError::Transport(e) => return self.dial_error_text(e),
            DbError::Connection(f) => {
                ErrorLog::new(self.paths.errors_log()).record("db.connection", f);
                network_reason(&f.kind)
            }
        };
        self.i18n.label(label).to_string()
    }

    /// Why a TLS connection was not made, in words; a fault's detail goes to the error log.
    pub(in crate::app) fn tls_problem_text(&self, p: &TlsProblem) -> String {
        let log = |f: &datarig_core::fault::Fault| ErrorLog::new(self.paths.errors_log()).record("db.tls", f);
        let file_name = |f: TlsFile| {
            let label = match f {
                TlsFile::Ca => Label::TlsFileCa,
                TlsFile::Cert => Label::TlsFileCert,
                TlsFile::Key => Label::TlsFileKey,
            };
            self.i18n.label(label).to_string()
        };
        let msg: Msg = match p {
            TlsProblem::UnknownMode(mode) => Msg::TlsUnknownMode { mode: mode.clone() },
            TlsProblem::DsnUnreadable => Label::TlsDsnUnreadable.into(),
            TlsProblem::NeedsCaFile => Label::TlsNeedsCa.into(),
            TlsProblem::CertWithoutKey => Label::TlsCertWithoutKey.into(),
            TlsProblem::KeyWithoutCert => Label::TlsKeyWithoutCert.into(),
            TlsProblem::PathRelative { file, path } => {
                Msg::TlsPathRelative { file: file_name(*file), path: path.clone() }
            }
            TlsProblem::File { file, path, problem } => {
                let (file, path) = (file_name(*file), path.clone());
                match problem {
                    FileProblem::Unreadable(f) => {
                        log(f);
                        let error = self.i18n.msg(&persist::fault_reason(f)).to_string();
                        Msg::TlsFileUnreadable { file, path, error }
                    }
                    FileProblem::NoCertificate => Msg::TlsFileNoCertificate { file, path },
                    FileProblem::NoKey => Msg::TlsFileNoKey { path },
                    FileProblem::Encrypted => Msg::TlsKeyEncrypted { path },
                    FileProblem::Invalid(f) => {
                        log(f);
                        Msg::TlsFileInvalid { file, path }
                    }
                }
            }
            TlsProblem::Unsupported => Label::TlsUnsupported.into(),
            TlsProblem::Untrusted => Label::TlsUntrusted.into(),
            TlsProblem::NameMismatch { host } => Msg::TlsNameMismatch { host: host.clone() },
            TlsProblem::HostNotCheckable { host } => Msg::TlsHostNotCheckable { host: host.clone() },
            TlsProblem::NotForServer => Label::TlsNotForServer.into(),
            TlsProblem::Expired => Label::TlsExpired.into(),
            TlsProblem::NotYetValid => Label::TlsNotYetValid.into(),
            TlsProblem::Revoked => Label::TlsRevoked.into(),
            TlsProblem::ClientCertRejected => Label::TlsClientCertRejected.into(),
            TlsProblem::PasswordUnverified => Label::TlsPasswordUnverified.into(),
            TlsProblem::SystemRoots(f) => {
                log(f);
                Label::TlsSystemRoots.into()
            }
            TlsProblem::Handshake(f) => {
                log(f);
                Label::TlsHandshake.into()
            }
        };
        self.i18n.msg(&msg).to_string()
    }

    /// A failure of the transport a session reaches its server through (a tunnel),
    /// in words; the far end's message or the fault's detail goes to the error log.
    pub(in crate::app) fn dial_error_text(&self, e: &DialError) -> String {
        let log = |f: &datarig_core::fault::Fault| ErrorLog::new(self.paths.errors_log()).record("db.transport", f);
        let msg = match e {
            DialError::NotOpen => return self.i18n.label(Label::DialNotOpen).to_string(),
            DialError::Timeout(d) => Msg::DialTimeout { elapsed: *d },
            DialError::Refused { host, port, reason, detail } => {
                let target = format!("{host}:{port}");
                // The far end's message may be empty: the target and the reason say what it was.
                let text = format!(
                    "{target} refused ({reason:?}){}",
                    if detail.is_empty() { String::new() } else { format!(": {detail}") }
                );
                log(&datarig_core::fault::Fault::other(text));
                match reason {
                    Refusal::Prohibited => Msg::DialProhibited { target },
                    Refusal::Unreachable => Msg::DialUnreachable { target },
                    Refusal::Other => Msg::DialRefused { target },
                }
            }
            // Reaching the far end failed: worded (and logged) as a connection failure is.
            DialError::Failed(f) => return self.db_error_text(&DbError::Connection(f.clone())),
        };
        self.i18n.msg(&msg).to_string()
    }
}

/// A network failure (connecting to a server or a bastion) in words: refused, cut, timed out,
/// unreachable, a name that does not resolve; anything else is a connection failure whose
/// detail is in `errors.log`. Never a file system wording.
pub(in crate::app) fn network_reason(kind: &FaultKind) -> Label {
    use std::io::ErrorKind as K;
    match kind {
        FaultKind::HostNotFound => Label::DbHostNotFound,
        FaultKind::Io(K::ConnectionRefused) => Label::DbRefused,
        FaultKind::Io(
            K::ConnectionReset | K::ConnectionAborted | K::BrokenPipe | K::UnexpectedEof | K::NotConnected,
        ) => Label::DbCut,
        FaultKind::Io(K::TimedOut) => Label::DbTimedOut,
        FaultKind::Io(K::HostUnreachable | K::NetworkUnreachable | K::NetworkDown | K::AddrNotAvailable) => {
            Label::DbUnreachable
        }
        _ => Label::DbConnection,
    }
}
