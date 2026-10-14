//! The few fields of a DER certificate datarig reads itself: its signature algorithm (channel
//! binding) and its validity (a CA file's own certificate used as the server's). Every read is
//! bounds-checked; anything unexpected is `None`.

/// The content of the DER element at the start of `der` with tag `tag`, and what follows it.
pub(crate) fn element(der: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
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

/// The element at the start of `der`, whatever its tag: its tag, content, and what follows.
fn any(der: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *der.first()?;
    let (content, rest) = element(der, tag)?;
    Some((tag, content, rest))
}

/// The OID (its content bytes) of a certificate's `signatureAlgorithm`:
/// `Certificate ::= SEQUENCE { tbsCertificate, signatureAlgorithm AlgorithmIdentifier, … }`,
/// `AlgorithmIdentifier ::= SEQUENCE { algorithm OBJECT IDENTIFIER, … }`.
pub(crate) fn signature_algorithm(der: &[u8]) -> Option<&[u8]> {
    let (certificate, _) = element(der, 0x30)?;
    let (_, rest) = element(certificate, 0x30)?;
    let (identifier, _) = element(rest, 0x30)?;
    let (oid, _) = element(identifier, 0x06)?;
    Some(oid)
}

/// A certificate's validity, `notBefore` and `notAfter` in seconds since the Unix epoch:
/// `TBSCertificate ::= SEQUENCE { [0] version OPTIONAL, serialNumber, signature, issuer,
/// validity SEQUENCE { notBefore Time, notAfter Time }, … }`.
pub(crate) fn validity(der: &[u8]) -> Option<(i64, i64)> {
    let (certificate, _) = element(der, 0x30)?;
    let (mut tbs, _) = element(certificate, 0x30)?;
    if tbs.first() == Some(&0xa0) {
        tbs = any(tbs)?.2;
    }
    let (_, _, tbs) = any(tbs)?; // serialNumber
    let (_, _, tbs) = any(tbs)?; // signature
    let (_, _, tbs) = any(tbs)?; // issuer
    let (validity, _) = element(tbs, 0x30)?;
    let (t1, b1, rest) = any(validity)?;
    let (t2, b2, _) = any(rest)?;
    Some((time(t1, b1)?, time(t2, b2)?))
}

/// A `UTCTime` (`YYMMDDHHMMSSZ`, years 1950–2049) or `GeneralizedTime` (`YYYYMMDDHHMMSSZ`) in
/// seconds since the Unix epoch.
fn time(tag: u8, text: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(text).ok()?.strip_suffix('Z')?;
    let (year, rest) = match (tag, text.len()) {
        (0x17, 12) => {
            let yy: i64 = text.get(..2)?.parse().ok()?;
            (if yy < 50 { 2000 + yy } else { 1900 + yy }, &text[2..])
        }
        (0x18, 14) => (text.get(..4)?.parse().ok()?, &text[4..]),
        _ => return None,
    };
    if !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = |i: usize| rest[i..i + 2].parse::<i64>().ok();
    let (month, day, h, m, s) = (n(0)?, n(2)?, n(4)?, n(6)?, n(8)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || h > 23 || m > 59 || s > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + s)
}

/// Days from 1970-01-01 to a date of the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
