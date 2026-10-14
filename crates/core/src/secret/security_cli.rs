//! [`SecretStore`] on the macOS keychain through Apple's `/usr/bin/security` tool
//! (`[secrets] macos_keychain_access = "security_tool"`).
//!
//! Why: release binaries are not signed, so after each upgrade macOS sees a new program, the
//! "Always Allow" given to the old one no longer matches, and the keychain asks for the login
//! password again. `security` is signed by Apple and stays the same across upgrades, so an
//! "Always Allow" given to it lasts. The price: any program running as the same user can read
//! these passwords through `security` without being asked.
//!
//! How:
//! * The program is always [`PROGRAM`] (never looked up on `PATH`). It runs in interactive mode
//!   (`security -i`), and the one command is written to its standard input as a line, which is
//!   then closed. **A password never appears in the arguments** (other users' `ps` would see
//!   them). Inside double quotes the tool reads `\"` and `\\` as escapes and nothing else, so
//!   every quoted word escapes exactly those two.
//! * A line longer than about 4000 bytes makes the tool hang; a request longer than
//!   [`LINE_LIMIT`] is refused before anything runs, as is a password with a line break or a
//!   NUL (it cannot be written on one line). A write is read back once and must give the same
//!   password; a write that prints anything on standard error has failed, and a delete that does
//!   (it reports what it removed) counts only when a read then finds nothing.
//! * The tool runs in a process group of its own, never the terminal's foreground group.
//! * A read (`find-generic-password … -g`) gets the password on standard error, in one of three
//!   shapes: `password: ` (empty), `password: "<printable ASCII>"`, or
//!   `password: 0x<hex>  "<escaped>"` (any other byte, or a backslash). Anything else is an
//!   error, never "no password".
//! * A failed command says `<command>: returned <OSStatus>` on standard error. Only
//!   `errSecItemNotFound` (-25300) means "no entry"; every other failure is a fault whose detail
//!   holds the tool's own `security: …` lines and the status, never the output of a read (it
//!   holds the password) nor a line with the password in it.
//! * The items are the same as the direct store's (service `datarig`, account `profile:<id>`,
//!   the default keychain), so switching between the two moves nothing.
//! * Each call waits for the tool. It is not stopped after [`super::KEYCHAIN_TIMEOUT`]: it may
//!   be showing a macOS permission dialog the user is answering. [`super::Guarded`] already
//!   tells the UI that the keychain did not answer in time.

use super::{SERVICE, SecretStore, Unavailable};
use crate::fault::{Fault, FaultKind, KeychainFault};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// The `security` tool every production call runs.
pub const PROGRAM: &str = "/usr/bin/security";

/// The longest request line (with its newline) written to the tool. The tool hangs on lines of
/// about 4000 bytes; this leaves room to spare.
pub const LINE_LIMIT: usize = 3500;

/// `errSecItemNotFound`: the only status that means "no such entry".
const NOT_FOUND: i32 = -25300;

/// The macOS keychain through [`PROGRAM`] (see the module docs).
pub struct SecurityCliStore {
    program: PathBuf,
    service: String,
    /// The keychain file to use; `None`: the default keychain and the search list, as the direct
    /// store.
    keychain: Option<PathBuf>,
    /// `HOME` for the tool (tests only: a scratch home, so the user's keychain settings are never
    /// read or changed).
    home: Option<PathBuf>,
}

impl SecurityCliStore {
    /// [`PROGRAM`], service [`SERVICE`], the default keychain.
    pub fn new() -> Self {
        Self { program: PathBuf::from(PROGRAM), service: SERVICE.to_string(), keychain: None, home: None }
    }

    /// Another program (a fake tool, or a wrapper that logs its arguments), service, keychain
    /// file and `HOME`, for tests.
    #[cfg(test)]
    pub(crate) fn with_tool(program: PathBuf, service: &str, keychain: Option<PathBuf>, home: Option<PathBuf>) -> Self {
        Self { program, service: service.to_string(), keychain, home }
    }

    /// The request line for `command` on `account`: `head` options before the service and
    /// account, `tail` after them (each `(option, value)`; a `None` value is a bare flag), the
    /// keychain last. Refused when a word cannot be written on one line or the line is too long.
    fn line(
        &self,
        command: &str,
        head: &[&str],
        account: &str,
        tail: &[(&str, Option<&str>)],
    ) -> Result<String, Unavailable> {
        let unwritable = |w: &str| w.contains(['\n', '\r', '\0']);
        if tail.iter().filter_map(|(_, v)| *v).any(unwritable) {
            return Err(refused("a password with a line break or NUL cannot be passed to the security tool"));
        }
        let keychain = match &self.keychain {
            Some(k) => Some(k.to_str().ok_or_else(|| refused("the keychain path is not UTF-8"))?),
            None => None,
        };
        if [self.service.as_str(), account].into_iter().chain(keychain).any(unwritable) {
            return Err(refused("a name with a line break or NUL cannot be passed to the security tool"));
        }
        let mut line = command.to_string();
        for h in head {
            line.push(' ');
            line.push_str(h);
        }
        line.push_str(&format!(" -s {} -a {}", quote(&self.service), quote(account)));
        for (option, value) in tail {
            line.push(' ');
            line.push_str(option);
            if let Some(v) = value {
                line.push(' ');
                line.push_str(&quote(v));
            }
        }
        if let Some(k) = keychain {
            line.push(' ');
            line.push_str(&quote(k));
        }
        line.push('\n');
        if line.len() > LINE_LIMIT {
            return Err(refused("request too long for the security tool"));
        }
        Ok(line)
    }

    /// Run the tool in interactive mode with `line` as its only input. Its standard input is a
    /// pipe closed after the line (never the terminal's).
    fn run(&self, line: &str) -> Result<Output, Unavailable> {
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new(&self.program);
        cmd.arg("-i").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // A process group of its own: it is never the terminal's foreground group, so it cannot
        // read the TUI's keys (nor get its Ctrl+C).
        cmd.process_group(0);
        if let Some(home) = &self.home {
            cmd.env("HOME", home);
        }
        let mut child = cmd.spawn().map_err(|e| {
            let detail = format!("{}: {e}", self.program.display());
            Unavailable(Fault::new(FaultKind::Keychain(KeychainFault::NoTool), detail))
        })?;
        // Dropped (closed) right after the line: the tool ends at the end of its input.
        let wrote = child.stdin.take().map(|mut stdin| stdin.write_all(line.as_bytes()));
        let out = child.wait_with_output().map_err(|e| failure(format!("waiting for the security tool: {e}")))?;
        if let Some(Err(e)) = wrote {
            return Err(failure(format!("the request could not be handed to the security tool: {e}")));
        }
        Ok(out)
    }
}

impl Default for SecurityCliStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for SecurityCliStore {
    fn get(&self, account: &str) -> Result<Option<String>, Unavailable> {
        const COMMAND: &str = "find-generic-password";
        let line = self.line(COMMAND, &[], account, &[("-g", None)])?;
        let out = self.run(&line)?;
        match verdict(&out, COMMAND) {
            Verdict::Done => password(&out.stderr).map(Some),
            Verdict::Status(NOT_FOUND) => Ok(None),
            v => Err(v.fault(COMMAND, &out, None)),
        }
    }

    /// Written, then read back once: only the same password back is a success (the tool's
    /// interactive mode might take a request it did not carry out without saying so).
    fn set(&self, account: &str, secret: &str) -> Result<(), Unavailable> {
        const COMMAND: &str = "add-generic-password";
        // `-U`: update the item when there is one.
        let line = self.line(COMMAND, &["-U"], account, &[("-w", Some(secret))])?;
        let out = self.run(&line)?;
        match quiet(verdict(&out, COMMAND), &out) {
            Verdict::Done => {}
            v => return Err(v.fault(COMMAND, &out, Some(secret))),
        }
        match self.get(account)? {
            Some(back) if back == secret => Ok(()),
            Some(_) => Err(mismatch("the security tool gave back a different password")),
            None => Err(mismatch("the password was not there when read back")),
        }
    }

    fn delete(&self, account: &str) -> Result<bool, Unavailable> {
        const COMMAND: &str = "delete-generic-password";
        let line = self.line(COMMAND, &[], account, &[])?;
        let out = self.run(&line)?;
        match quiet(verdict(&out, COMMAND), &out) {
            Verdict::Done => Ok(true),
            // The tool reports a delete on standard error when it works: only a read that finds
            // nothing any more makes that a success.
            Verdict::Noise => match self.get(account)? {
                None => Ok(true),
                Some(_) => Err(Verdict::Noise.fault(COMMAND, &out, None)),
            },
            Verdict::Status(NOT_FOUND) => Ok(false),
            v => Err(v.fault(COMMAND, &out, None)),
        }
    }
}

/// `s` as one double-quoted word of the tool: `\` and `"` escaped, nothing else changed.
pub(crate) fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// How a run of the tool ended.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// The command succeeded.
    Done,
    /// The command failed with this OSStatus (`<command>: returned <status>`).
    Status(i32),
    /// The tool exited with this code and named no status.
    Exit(i32),
    /// The tool was ended by a signal.
    Signal,
    /// The tool said it succeeded but wrote something on standard error (a write says nothing
    /// when it works).
    Noise,
}

/// `v`, except that a success with something on standard error is [`Verdict::Noise`].
fn quiet(v: Verdict, out: &Output) -> Verdict {
    match v {
        Verdict::Done if !String::from_utf8_lossy(&out.stderr).trim().is_empty() => Verdict::Noise,
        v => v,
    }
}

/// `line` holds `secret`: as given, as the tool's quoted word, or as the escaped text inside it.
fn holds(line: &str, secret: &str) -> bool {
    if secret.is_empty() {
        return false;
    }
    let quoted = quote(secret);
    let inner = &quoted[1..quoted.len() - 1];
    line.contains(secret) || line.contains(&quoted) || line.contains(inner)
}

/// The verdict of a run of `command`. The status line is looked for only at the start of a line
/// that begins with the command's name, so a password line can never be taken for one.
fn verdict(out: &Output, command: &str) -> Verdict {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let returned = stderr.lines().find_map(|l| {
        let n = l.strip_prefix(command)?.strip_prefix(": returned ")?;
        n.trim().parse::<i32>().ok()
    });
    match (returned, out.status.code()) {
        (Some(0), Some(0)) => Verdict::Done,
        (Some(0), Some(c)) => Verdict::Exit(c),
        (Some(s), _) => Verdict::Status(s),
        (None, Some(0)) => Verdict::Done,
        (None, Some(c)) => Verdict::Exit(c),
        (None, None) => Verdict::Signal,
    }
}

impl Verdict {
    /// The fault of a failed run: its kind by status, and as detail the tool's own `security: `
    /// lines (none that holds `secret`) and the status or exit code. Never the rest of the
    /// output.
    fn fault(self, command: &str, out: &Output, secret: Option<&str>) -> Unavailable {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let said: Vec<&str> = stderr
            .lines()
            .map(str::trim_end)
            .filter(|l| l.starts_with("security: "))
            .filter(|l| secret.is_none_or(|s| !holds(l, s)))
            .collect();
        let said = said.join(" | ");
        let (kind, why) = match self {
            Verdict::Status(s) => (status_fault(s), format!("OSStatus {s}")),
            Verdict::Exit(c) => (KeychainFault::Failure, format!("exit code {c}")),
            Verdict::Signal => (KeychainFault::Failure, "ended by a signal".to_string()),
            Verdict::Noise => (KeychainFault::Failure, "unexpected output from the security tool".to_string()),
            Verdict::Done => (KeychainFault::Failure, "no failure".to_string()),
        };
        let detail = match said.as_str() {
            "" => format!("{command} failed ({why})"),
            s => format!("{s} ({why})"),
        };
        Unavailable(Fault::new(FaultKind::Keychain(kind), detail))
    }
}

/// The kind of failure an OSStatus of the tool means.
fn status_fault(status: i32) -> KeychainFault {
    match status {
        // userCanceled, errSecAuthFailed, errSecInteractionNotAllowed.
        -128 | -25293 | -25308 => KeychainFault::Locked,
        // errSecNoSuchKeychain, errSecNoDefaultKeychain.
        -25294 | -25307 => KeychainFault::NoStore,
        // errSecDuplicateItem.
        -25299 => KeychainFault::Ambiguous,
        // errSecDecode.
        -26275 => KeychainFault::Damaged,
        _ => KeychainFault::Failure,
    }
}

/// The password of a successful read: the one `password: ` line of standard error.
fn password(stderr: &[u8]) -> Result<String, Unavailable> {
    let unexpected = || failure("unexpected output from the security tool");
    let mut lines = stderr.split(|&b| b == b'\n').filter_map(|l| l.strip_prefix(b"password:"));
    let (Some(rest), None) = (lines.next(), lines.next()) else { return Err(unexpected()) };
    let rest = match rest {
        [] => return Ok(String::new()),
        [b' ', rest @ ..] => rest,
        _ => return Err(unexpected()),
    };
    match rest {
        [] => Ok(String::new()),
        [b'0', b'x', hex @ ..] => {
            let end = hex.iter().position(|&b| b == b' ').unwrap_or(hex.len());
            let bytes = decode_hex(&hex[..end]).ok_or_else(unexpected)?;
            String::from_utf8(bytes).map_err(|_| {
                Unavailable(Fault::new(
                    FaultKind::Keychain(KeychainFault::Damaged),
                    "the stored password is not UTF-8 text",
                ))
            })
        }
        // The whole rest between the first and the last quote (the password may hold quotes).
        [b'"', inner @ .., b'"'] if inner.iter().all(|&b| (0x20..0x7f).contains(&b) && b != b'\\') => {
            Ok(String::from_utf8_lossy(inner).into_owned())
        }
        _ => Err(unexpected()),
    }
}

/// `hex` (an even number of hex digits, at least two) as bytes.
fn decode_hex(hex: &[u8]) -> Option<Vec<u8>> {
    if hex.is_empty() || !hex.len().is_multiple_of(2) {
        return None;
    }
    let digit = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    hex.chunks(2).map(|p| Some(digit(p[0])? << 4 | digit(p[1])?)).collect()
}

fn refused(detail: &str) -> Unavailable {
    Unavailable(Fault::new(FaultKind::Keychain(KeychainFault::Refused), detail))
}

fn mismatch(detail: &str) -> Unavailable {
    Unavailable(Fault::new(FaultKind::Keychain(KeychainFault::Mismatch), detail))
}

fn failure(detail: impl Into<String>) -> Unavailable {
    Unavailable(Fault::new(FaultKind::Keychain(KeychainFault::Failure), detail))
}

#[cfg(test)]
mod tests;
