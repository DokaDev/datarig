//! Testing a connection or a tunnel from a form: its state and the notice it ends with.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestState {
    Running,
    Ok(PingInfo),
    Failed(String),
    Timeout(Duration),
    Cancelled,
    /// The password source failed before the server was asked.
    Source(SourceError),
    /// A tunnel preset's test: what it reached through the tunnel, each address with its time
    /// or why not (in words).
    Reached(Vec<(String, Result<Duration, String>)>),
    /// A session preset's test: what its session said, at this level.
    Applied(Msg, Level),
}

pub struct ConnTest {
    pub seq: u64,
    pub started: Instant,
    pub state: TestState,
    pub(super) abort: Option<AbortHandle>,
    /// Through the profile's SSH tunnel: how far that got.
    pub tunnel: Option<TunnelTest>,
    /// A tunnel preset's test: after the tunnel, channels to its profiles' database addresses
    /// are tried instead of a database.
    pub probe: bool,
}

/// The SSH tunnel of a test connection: each stage shows while it runs,
/// and the tunnel's time is reported apart from the database's.
pub struct TunnelTest {
    pub host: String,
    /// The tested profile's name and tunnel settings (the questions it asks name them).
    pub name: String,
    pub settings: datarig_core::profile::ssh::SshSettings,
    pub stage: Option<datarig_ssh::tunnel::Stage>,
    /// The tunnel opened after this long.
    pub opened: Option<Duration>,
    /// How it opened: the host key taken and the login that worked.
    pub how: Option<datarig_ssh::tunnel::Opened>,
    /// Why it did not open (the SSH stage's line).
    pub failure: Option<String>,
}

pub(super) fn test_msg(i18n: &I18n, t: &ConnTest) -> Notice {
    let (state, started) = (&t.state, t.started);
    // Through a tunnel: the tunnel's stage while it opens, then its time next to the database's
    // result, so it shows which one failed.
    if let Some(tt) = &t.tunnel {
        let host = tt.host.clone();
        match (state, tt.opened) {
            (TestState::Running, Some(ssh)) if t.probe => {
                return Notice::new(Msg::TestSshProbing { host, elapsed: ssh }, Level::Info);
            }
            (TestState::Reached(v), Some(ssh)) if v.is_empty() => {
                return Notice::new(Msg::TestTunnelOkNoTargets { host, elapsed: ssh }, Level::Success);
            }
            (TestState::Reached(v), Some(ssh)) => {
                return match reached_failures(v) {
                    None => Notice::new(
                        Msg::TestTunnelOk { host, elapsed: ssh, targets: reached_targets(v) },
                        Level::Success,
                    ),
                    Some(error) => Notice::new(Msg::TestTunnelProbeFailed { host, elapsed: ssh, error }, Level::Error),
                };
            }
            (TestState::Running, None) => {
                let stage = i18n.label(tunnel::stage_label(tt.stage.unwrap_or(datarig_ssh::tunnel::Stage::Connecting)));
                return Notice::new(
                    Msg::TestSshRunning { host, stage: stage.to_string(), elapsed: started.elapsed() },
                    Level::Info,
                );
            }
            (TestState::Running, Some(ssh)) => {
                return Notice::new(Msg::TestSshDb { host, elapsed: ssh }, Level::Info);
            }
            (TestState::Ok(info), Some(ssh)) => {
                // Plain through the tunnel: the tunnel encrypts it as far as the bastion.
                let tls = match info.encryption {
                    Encryption::Plain => i18n.label(Label::TlsPlainTunnel).to_string(),
                    e => encryption_text(i18n, e),
                };
                let (version, latency) = (info.server_version.clone(), info.latency);
                return Notice::new(Msg::TestOkSsh { host, elapsed: ssh, version, latency, tls }, Level::Success);
            }
            (TestState::Failed(e), Some(ssh)) => {
                return Notice::new(Msg::TestFailedDb { host, elapsed: ssh, error: e.clone() }, Level::Error);
            }
            _ => {}
        }
    }
    match state {
        TestState::Running => Notice::new(Msg::TestRunning { elapsed: started.elapsed() }, Level::Info),
        TestState::Ok(info) => {
            let tls = encryption_text(i18n, info.encryption);
            Notice::new(
                Msg::TestOk { version: info.server_version.clone(), latency: info.latency, tls },
                Level::Success,
            )
        }
        TestState::Failed(e) => Notice::new(Msg::TestFailed { error: e.clone() }, Level::Error),
        TestState::Timeout(d) => Notice::new(Msg::TestTimeout { elapsed: *d }, Level::Error),
        TestState::Cancelled => Notice::new(Label::TestCancelled, Level::Warning),
        TestState::Source(e) => Notice::new(password::source_error_msg(i18n, e), Level::Error),
        TestState::Reached(v) => match reached_failures(v) {
            None => Notice::new(Msg::TestReached { targets: reached_targets(v) }, Level::Success),
            Some(error) => Notice::new(Msg::TestFailed { error }, Level::Error),
        },
        TestState::Applied(m, level) => Notice::new(m.clone(), *level),
    }
}

/// How a connection's traffic goes, in words: `TLS 1.3`, `TLS 1.3, certificate not checked`,
/// `not encrypted`.
pub(crate) fn encryption_text(i18n: &I18n, e: Encryption) -> String {
    match e {
        Encryption::Plain => i18n.label(Label::TlsPlain).to_string(),
        Encryption::Tls { version, verified } => {
            let version = version.label().to_string();
            match verified {
                Verified::Full => i18n.msg(&Msg::TlsVerified { version }).to_string(),
                Verified::Chain => i18n.msg(&Msg::TlsChain { version }).to_string(),
                Verified::No => i18n.msg(&Msg::TlsUnverified { version }).to_string(),
            }
        }
    }
}

/// The addresses a preset's test reached: `host:port, …`.
pub(super) fn reached_targets(v: &[(String, Result<Duration, String>)]) -> String {
    v.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(", ")
}

/// The addresses a preset's test did not reach, each with why (`None`: it reached them all).
pub(super) fn reached_failures(v: &[(String, Result<Duration, String>)]) -> Option<String> {
    let failed: Vec<String> = v.iter().filter_map(|(t, r)| r.as_ref().err().map(|e| format!("{t}: {e}"))).collect();
    (!failed.is_empty()).then(|| failed.join("; "))
}
