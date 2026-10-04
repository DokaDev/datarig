//! Editing a query in the user's own editor (`Ctrl+G`): the text goes to a private file in the
//! state directory, the editor named by `$VISUAL`, else `$EDITOR`, else `vi` (`notepad` on
//! Windows) runs on it in the terminal, and what it saved comes back.
//!
//! The command is split into words as a shell would (`code -w`, quotes), but no shell runs
//! it: the file's path is one more argument. The file is created new (never over an existing
//! one), readable by its owner only, and removed afterwards. Unknown is not absent: a variable
//! that does not split, an editor that fails or a file that cannot be read back each say why,
//! and the text stays as it was.
//!
//! The binary hands the terminal over around the editor ([`Handover`]); tests use a fake one
//! and a script in place of the editor.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};

/// The editor when neither `$VISUAL` nor `$EDITOR` names one.
pub const DEFAULT_EDITOR: &str = if cfg!(windows) { "notepad" } else { "vi" };

/// The command that edits a file, as the environment names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorCommand {
    /// The variable it came from (`VISUAL`, `EDITOR`), `None` for the default.
    pub var: Option<&'static str>,
    pub program: String,
    pub args: Vec<String>,
}

/// Why nothing came back from the editor; the text stays as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditFailure {
    /// There is no state directory for the file (no home directory is known).
    NoStateDir,
    /// The file for the editor could not be made.
    File(String),
    /// `$VISUAL` / `$EDITOR` does not split into words (an open quote).
    Command { var: &'static str, error: String },
    /// The editor did not start.
    Spawn { program: String, error: String },
    /// The editor ended with a failure (Vim's `:cq`), or was killed.
    Exit { program: String, how: Ended },
    /// The file could not be read back.
    Read(String),
    /// What the editor saved is not UTF-8.
    NotUtf8,
}

/// How an editor that failed ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    Code(i32),
    Signal(i32),
    /// Neither (the platform's own words).
    Other(String),
}

/// What came back from the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edited {
    /// New text.
    Changed(String),
    /// The file is as it was written (quit without saving, or saved unchanged).
    Unchanged,
    Failed(EditFailure),
}

/// Gives the terminal to the editor and takes it back: the binary leaves the alternate screen
/// and raw mode, then sets them up again. Taking it back is called whenever giving it away
/// was, however the editor ended.
pub trait Handover {
    fn release(&mut self) -> io::Result<()>;
    fn reclaim(&mut self) -> io::Result<()>;
}

/// The editor `env` names: `$VISUAL`, else `$EDITOR`, else [`DEFAULT_EDITOR`]. An empty or blank
/// variable counts as unset; one that does not split is an error (it is not skipped).
pub fn editor_command(env: impl Fn(&str) -> Option<String>) -> Result<EditorCommand, EditFailure> {
    for var in ["VISUAL", "EDITOR"] {
        let Some(value) = env(var) else { continue };
        let words = shell_words::split(&value).map_err(|e| EditFailure::Command { var, error: e.to_string() })?;
        let mut words = words.into_iter();
        if let Some(program) = words.next() {
            return Ok(EditorCommand { var: Some(var), program, args: words.collect() });
        }
    }
    Ok(EditorCommand { var: None, program: DEFAULT_EDITOR.to_string(), args: Vec::new() })
}

/// Edit `text` with `cmd` in a new file under `dir` (`None`: there is no state directory).
/// `Err` only when the terminal could not be taken back; everything else is an [`Edited`].
pub fn edit(dir: Option<&Path>, text: &str, cmd: &EditorCommand, term: &mut impl Handover) -> io::Result<Edited> {
    let Some(dir) = dir else { return Ok(Edited::Failed(EditFailure::NoStateDir)) };
    let written = format!("{text}\n");
    let file = match TempFile::create(dir, written.as_bytes()) {
        Ok(f) => f,
        Err(e) => return Ok(Edited::Failed(EditFailure::File(e.to_string()))),
    };
    let mut command = Command::new(&cmd.program);
    command.args(&cmd.args).arg(&file.path);
    term.release()?;
    let status = command.status();
    term.reclaim()?;
    let status = match status {
        Ok(s) => s,
        Err(e) => {
            return Ok(Edited::Failed(EditFailure::Spawn { program: cmd.program.clone(), error: e.to_string() }));
        }
    };
    if !status.success() {
        return Ok(Edited::Failed(EditFailure::Exit { program: cmd.program.clone(), how: ended(status) }));
    }
    let bytes = match fs::read(&file.path) {
        Ok(b) => b,
        Err(e) => return Ok(Edited::Failed(EditFailure::Read(e.to_string()))),
    };
    Ok(read_back(text, written.as_bytes(), bytes))
}

/// What the file holding `written` (the text `text` and a line break) says now.
fn read_back(text: &str, written: &[u8], bytes: Vec<u8>) -> Edited {
    if bytes == written {
        return Edited::Unchanged;
    }
    let Ok(s) = String::from_utf8(bytes) else { return Edited::Failed(EditFailure::NotUtf8) };
    // Line breaks as a paste takes them; the line break that ends the file is not a line.
    let s = s.replace("\r\n", "\n").replace('\r', "\n");
    let s = s.strip_suffix('\n').unwrap_or(&s);
    if s == text { Edited::Unchanged } else { Edited::Changed(s.to_string()) }
}

/// How the editor ended: its exit code, or the signal that ended it.
fn ended(status: ExitStatus) -> Ended {
    if let Some(code) = status.code() {
        return Ended::Code(code);
    }
    #[cfg(unix)]
    if let Some(sig) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return Ended::Signal(sig);
    }
    Ended::Other(status.to_string())
}

/// A file only its owner reads, removed when dropped.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn create(dir: &Path, bytes: &[u8]) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        private_dir(dir)?;
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let path = dir.join(format!("query-{}-{n}.sql", std::process::id()));
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&path)?;
        let file = TempFile { path };
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(file)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Make `dir` (and its parents), private to this user: `0700` when this user owns it, refused
/// when another user does.
fn private_dir(dir: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let meta = fs::metadata(dir)?;
        let uid = rustix::process::geteuid().as_raw();
        if meta.uid() != uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is owned by user {}, not {uid}", dir.display(), meta.uid()),
            ));
        }
        if meta.mode() & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
