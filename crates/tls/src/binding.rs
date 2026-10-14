//! PostgreSQL's SCRAM channel binding over TLS (`tls-server-end-point`, RFC 5929): the hash of
//! the server's certificate, with the hash function of the certificate's own signature (SHA-256
//! for MD5 and SHA-1). A certificate signed otherwise (RSASSA-PSS, Ed25519) has no binding here:
//! the login then goes without one, as libpq's does when it cannot compute it.

use ring::digest;
use tokio_rustls::client::TlsStream;

/// The `tls-server-end-point` data of a TLS connection, if its server certificate has one.
pub fn server_end_point<S>(stream: &TlsStream<S>) -> Option<Vec<u8>> {
    let cert = stream.get_ref().1.peer_certificates()?.first()?;
    end_point(cert.as_ref())
}

/// The binding data of a DER certificate.
pub(crate) fn end_point(der: &[u8]) -> Option<Vec<u8>> {
    let algorithm = match signature_algorithm(der)? {
        // md5WithRSAEncryption, sha1WithRSAEncryption, sha256WithRSAEncryption, ecdsa-with-SHA1,
        // ecdsa-with-SHA256.
        [42, 134, 72, 134, 247, 13, 1, 1, 4 | 5 | 11]
        | [42, 134, 72, 206, 61, 4, 1]
        | [42, 134, 72, 206, 61, 4, 3, 2] => &digest::SHA256,
        // sha384WithRSAEncryption, ecdsa-with-SHA384.
        [42, 134, 72, 134, 247, 13, 1, 1, 12] | [42, 134, 72, 206, 61, 4, 3, 3] => &digest::SHA384,
        // sha512WithRSAEncryption, ecdsa-with-SHA512.
        [42, 134, 72, 134, 247, 13, 1, 1, 13] | [42, 134, 72, 206, 61, 4, 3, 4] => &digest::SHA512,
        _ => return None,
    };
    Some(digest::digest(algorithm, der).as_ref().to_vec())
}

/// The OID (its content bytes) of a certificate's `signatureAlgorithm`:
/// `Certificate ::= SEQUENCE { tbsCertificate, signatureAlgorithm AlgorithmIdentifier, … }`,
/// `AlgorithmIdentifier ::= SEQUENCE { algorithm OBJECT IDENTIFIER, … }`.
fn signature_algorithm(der: &[u8]) -> Option<&[u8]> {
    let (certificate, _) = element(der, 0x30)?;
    let (_, rest) = element(certificate, 0x30)?;
    let (identifier, _) = element(rest, 0x30)?;
    let (oid, _) = element(identifier, 0x06)?;
    Some(oid)
}

/// The content of the DER element at the start of `der` with tag `tag`, and what follows it.
fn element(der: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (&first, rest) = der.split_first()?;
    if first != tag {
        return None;
    }
    let (&len, mut rest) = rest.split_first()?;
    let len = match len {
        0..=0x7f => usize::from(len),
        0x81..=0x84 => {
            let n = usize::from(len & 0x7f);
            let bytes = rest.get(..n)?;
            rest = &rest[n..];
            bytes.iter().fold(0usize, |acc, b| (acc << 8) | usize::from(*b))
        }
        _ => return None,
    };
    let content = rest.get(..len)?;
    Some((content, &rest[len..]))
}
