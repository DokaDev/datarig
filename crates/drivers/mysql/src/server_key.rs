//! The server's public key from the profile's key file: an RSA public key in PEM, as MySQL
//! writes it (`public_key.pem`: `BEGIN PUBLIC KEY`, or PKCS#1's `BEGIN RSA PUBLIC KEY`).
//!
//! mysql_async encrypts the password with the key through mysql_common, whose parser panics
//! on anything but a well-formed key; the file is checked here first, with mysql_async's
//! check of a key (the one it also makes of a key the server sends).

use datarig_core::driver::DbError;
use datarig_core::fault::Fault;

/// The key file `path` names (`~/` is the home directory), checked. A path that neither
/// starts with `~/` nor has a root is refused: it would be read from whatever directory
/// datarig was started in.
pub(crate) fn read(path: &str) -> Result<Vec<u8>, DbError> {
    if !path.starts_with("~/") && !std::path::Path::new(path).has_root() {
        return Err(DbError::ServerKeyPathRelative(path.to_string()));
    }
    let file = match (path.strip_prefix("~/"), std::env::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => std::path::PathBuf::from(path),
    };
    let error = |fault| DbError::ServerKeyFile { path: path.to_string(), fault };
    let pem = std::fs::read(&file).map_err(|e| error(Some(Fault::io(&e))))?;
    match mysql_async::server_public_key_ok(&pem) {
        true => Ok(pem),
        false => Err(error(None)),
    }
}
