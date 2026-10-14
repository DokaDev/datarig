//! `password_source = "command"`: run the user's command and use its
//! standard output as the password.
//!
//! * The command line is split with POSIX shell rules (quotes, backslashes; the
//!   `shell-words` crate) and the program is run **directly, without a shell**: no pipes,
//!   `$VARS`, `~` or globs. To use those, ask for a shell explicitly:
//!   `sh -c 'pass show db/prod | head -n1'`.
//! * Standard input is closed; the command gets [`TIMEOUT`] and is killed after it.
//! * One trailing newline (`\n` or `\r\n`) is removed from the output.
//! * A non-zero exit fails with the exit status and the start of standard error
//!   ([`STDERR_LIMIT`] characters). Standard output is never logged or shown.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How long a password command may run.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// How much of standard error a failure shows.
pub const STDERR_LIMIT: usize = 200;

/// Why a command gave no password.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    /// Nothing to run.
    Empty,
    /// The command line could not be split: a quote is not closed (the only way
    /// `shell-words` fails).
    Parse,
    /// The program could not be started (not found, not executable, …).
    Start { program: String, error: crate::fault::Fault },
    /// Still running after the timeout (it was killed).
    Timeout(Duration),
    /// It exited unsuccessfully. `stderr` is trimmed and cut to [`STDERR_LIMIT`] characters.
    Failed { status: String, stderr: String },
    /// It succeeded but printed nothing.
    NoOutput,
    /// Its output is not UTF-8.
    NotUtf8,
}

/// Split `line` into the program and its arguments (POSIX shell quoting, no expansion).
pub fn split(line: &str) -> Result<Vec<String>, CommandError> {
    let words = shell_words::split(line).map_err(|_| CommandError::Parse)?;
    if words.is_empty() {
        return Err(CommandError::Empty);
    }
    Ok(words)
}

/// Run `line` and return its output without one trailing newline. Blocking (up to `timeout`).
pub fn run(line: &str, timeout: Duration) -> Result<String, CommandError> {
    let words = split(line)?;
    let mut child = Command::new(&words[0])
        .args(&words[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CommandError::Start { program: words[0].clone(), error: e.into() })?;
    // Drain both pipes on their own threads so a chatty command never blocks on a full pipe.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                // The reader threads end when the pipes close; they are not waited for (a
                // grandchild may still hold them open).
                return Err(CommandError::Timeout(timeout));
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(CommandError::Start { program: words[0].clone(), error: e.into() }),
        }
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if !status.success() {
        let status = status.code().map_or_else(|| status.to_string(), |c| c.to_string());
        return Err(CommandError::Failed { status, stderr: truncate(String::from_utf8_lossy(&stderr).trim()) });
    }
    let text = String::from_utf8(stdout).map_err(|_| CommandError::NotUtf8)?;
    let pw = trim_newline(&text);
    if pw.is_empty() {
        return Err(CommandError::NoOutput);
    }
    Ok(pw.to_string())
}

/// `s` without one trailing `\n` or `\r\n`.
pub fn trim_newline(s: &str) -> &str {
    s.strip_suffix("\r\n").or_else(|| s.strip_suffix('\n')).unwrap_or(s)
}

fn truncate(s: &str) -> String {
    match s.char_indices().nth(STDERR_LIMIT) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests;
