//! The server's public key from the profile's key file: an RSA public key in PEM, as MySQL
//! writes it (`public_key.pem`: `BEGIN PUBLIC KEY`, or PKCS#1's `BEGIN RSA PUBLIC KEY`).
//!
//! mysql_async encrypts the password with the key through mysql_common, whose parser panics
//! on anything but a well-formed key; the file is checked here first, the way that parser
//! reads it, so a key that passes is one it reads.

use datarig_core::driver::DbError;
use datarig_core::fault::Fault;

/// The shortest key taken, in bits (MySQL makes 2048-bit keys).
const MIN_BITS: usize = 2048;

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
    match rsa_bits(&pem) {
        Some(bits) if bits >= MIN_BITS => Ok(pem),
        _ => Err(error(None)),
    }
}

/// The size of the RSA key in `pem` in bits, when mysql_common reads it: the first PKCS#1
/// block, else the first SubjectPublicKeyInfo block (a block's text has no `-`), its base64,
/// and the DER structure its parser walks.
fn rsa_bits(pem: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(pem).ok()?;
    let pkcs1 = blocks(text, "-----BEGIN RSA PUBLIC KEY-----", "-----END RSA PUBLIC KEY-----").next();
    let (body, pkcs1) = match pkcs1 {
        Some(b) => (b, true),
        None => (blocks(text, "-----BEGIN PUBLIC KEY-----", "-----END PUBLIC KEY-----").next()?, false),
    };
    let body: String = body.chars().filter(|c| !" \n\t\r\x0b\x0c".contains(*c)).collect();
    let der = data_encoding::BASE64.decode(body.as_bytes()).ok()?;
    let key = if pkcs1 {
        der.as_slice()
    } else {
        let (info, _) = element(&der, 0x30)?;
        let (_algorithm, rest) = element(info, 0x30)?;
        let (bits, _) = element(rest, 0x03)?;
        // No unused bits, then the PKCS#1 key.
        match bits.split_first()? {
            (0, key) => key,
            _ => return None,
        }
    };
    let (fields, _) = element(key, 0x30)?;
    let (modulus, rest) = element(fields, 0x02)?;
    element(rest, 0x02)?;
    let modulus = &modulus[modulus.iter().position(|b| *b != 0)?..];
    Some(modulus.len() * 8 - modulus[0].leading_zeros() as usize)
}

/// The texts between `begin` and `end` without a `-` in them, in order.
fn blocks<'a>(text: &'a str, begin: &'a str, end: &'a str) -> impl Iterator<Item = &'a str> {
    text.match_indices(begin).filter_map(move |(i, _)| {
        let after = &text[i + begin.len()..];
        let body = &after[..after.find('-').unwrap_or(after.len())];
        after[body.len()..].starts_with(end).then_some(body)
    })
}

/// A DER element with tag `tag` at the start of `der`: its contents and what follows.
fn element(der: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (&t, rest) = der.split_first()?;
    if t != tag {
        return None;
    }
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first & 0x80 == 0 {
        (usize::from(first), rest)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        let len = rest[..n].iter().fold(0usize, |acc, b| acc << 8 | usize::from(*b));
        (len, &rest[n..])
    };
    (rest.len() >= len).then(|| rest.split_at(len))
}
