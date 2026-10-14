//! Private key files: OpenSSH's format (`BEGIN OPENSSH PRIVATE KEY`, ed25519, ECDSA, RSA) and
//! PEM, as AWS and `openssl` write them (`BEGIN RSA PRIVATE KEY` PKCS#1, `BEGIN PRIVATE KEY`
//! PKCS#8, `BEGIN EC PRIVATE KEY`), encrypted or not (`BEGIN ENCRYPTED PRIVATE KEY`, a PEM
//! `Proc-Type: 4,ENCRYPTED` header, an OpenSSH key with a passphrase). A certificate next to
//! the key (`<file>-cert.pub`) is used too. PuTTY keys are named as such (convert them with
//! `puttygen`).
//!
//! Like OpenSSH, a key file that others may read is refused (Unix): `chmod 600` it.

use datarig_core::fault::Fault;
use russh::keys::{Certificate, PrivateKey};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A key read from its file, and its certificate when there is one.
#[derive(Clone, Debug)]
pub struct LoadedKey {
    pub key: Arc<PrivateKey>,
    pub cert: Option<Certificate>,
}

/// Why a key file cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// The file cannot be read (not there, no permission).
    Unreadable(Fault),
    /// Others may read it (Unix mode bits, shown in octal).
    OpenToOthers { mode: u32 },
    /// A PuTTY key (`.ppk`).
    Putty,
    /// It is encrypted and no passphrase was given.
    Encrypted,
    /// The passphrase does not open it.
    WrongPassphrase,
    /// Not a private key datarig can read; the parser's words, for the error log.
    Unsupported(String),
    /// A legacy PEM key encrypted with a cipher datarig cannot open (only `AES-128-CBC` is
    /// read; `ssh-keygen -p` re-encrypts it in OpenSSH's format): the cipher's name.
    UnsupportedEncryption(String),
}

/// The certificate that goes with a key file (`<file>-cert.pub`).
pub fn cert_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-cert.pub");
    PathBuf::from(name)
}

/// The cipher of a legacy PEM key's `DEK-Info` header, when it is one datarig cannot open.
/// Told from the header alone, before any passphrase is asked for.
fn unsupported_cipher(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find_map(|l| l.strip_prefix("DEK-Info:"))?;
    let cipher = line.split(',').next()?.trim().to_string();
    (!cipher.eq_ignore_ascii_case("AES-128-CBC")).then_some(cipher)
}

/// Whether the key in `text` is encrypted, as far as its armor tells (an OpenSSH key says so
/// only when it is decoded).
fn looks_encrypted(text: &str) -> bool {
    text.contains("BEGIN ENCRYPTED PRIVATE KEY") || text.contains("Proc-Type: 4,ENCRYPTED")
}

/// Read the key at `path`, with `passphrase` when it is encrypted (a passphrase given for a key
/// that is not is ignored).
pub fn load(path: &Path, passphrase: Option<&str>) -> Result<LoadedKey, KeyError> {
    let bytes = std::fs::read(path).map_err(|e| KeyError::Unreadable(Fault::io_at(&e, path)))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode =
            std::fs::metadata(path).map_err(|e| KeyError::Unreadable(Fault::io_at(&e, path)))?.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(KeyError::OpenToOthers { mode: mode & 0o777 });
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    if text.trim_start().starts_with("PuTTY-User-Key-File") {
        return Err(KeyError::Putty);
    }
    if let Some(cipher) = unsupported_cipher(&text) {
        return Err(KeyError::UnsupportedEncryption(cipher));
    }
    let armored = looks_encrypted(&text);
    let key = match russh::keys::decode_secret_key(&text, None) {
        // Not encrypted: a passphrase given anyway is not needed.
        Ok(key) if !armored => key,
        Err(e) if !armored && !matches!(e, russh::keys::Error::KeyIsEncrypted) => {
            return Err(KeyError::Unsupported(e.to_string()));
        }
        _ => match passphrase {
            None => return Err(KeyError::Encrypted),
            Some(p) => russh::keys::decode_secret_key(&text, Some(p)).map_err(|_| KeyError::WrongPassphrase)?,
        },
    };
    let cert = std::fs::read_to_string(cert_path(path)).ok().and_then(|t| Certificate::from_openssh(t.trim()).ok());
    Ok(LoadedKey { key: Arc::new(key), cert })
}

#[cfg(test)]
mod tests;
