//! Names of saved queries. The user types a name only;
//! `a/b` makes a subfolder and the extension comes from the language. A name must be a
//! portable file path on every OS: no empty parts, no `.`/`..`, no hidden files, no
//! characters or names Windows refuses, no parts that end in a space or a dot.

/// The largest part of a name, in bytes (most file systems allow 255).
pub const MAX_PART: usize = 255;

/// Why a typed name cannot be a saved query's path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameError {
    /// Nothing typed.
    Empty,
    /// `a//b`, a leading or trailing `/`.
    EmptyPart,
    /// A part is `.` or `..`.
    Dots,
    /// A part starts with a `.` (a hidden file the list would not show).
    Hidden,
    /// A part starts or ends with a space, or ends with a dot (Windows drops them).
    Edges,
    /// A character no file name may have on Windows (`<>:"\|?*`) or a control character.
    Char(char),
    /// A name Windows reserves (`CON`, `NUL`, `COM1`, …), with or without an extension.
    Reserved(String),
    /// A part longer than [`MAX_PART`] bytes.
    TooLong,
    /// A saved query or folder with this name (ignoring case) exists: its path.
    Exists(String),
    /// A folder on the way cannot be read (its path, `""` for the scripts directory), so
    /// whether the name is taken is unknown.
    Unreadable(String),
}

const WINDOWS_CHARS: &[char] = &['<', '>', ':', '"', '\\', '|', '?', '*'];

fn windows_reserved(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

/// Check one part of a path (a folder or the file name).
pub fn check_part(part: &str) -> Result<(), NameError> {
    if part.is_empty() {
        return Err(NameError::EmptyPart);
    }
    if part == "." || part == ".." {
        return Err(NameError::Dots);
    }
    if part.starts_with('.') {
        return Err(NameError::Hidden);
    }
    if part.starts_with(char::is_whitespace) || part.ends_with(char::is_whitespace) || part.ends_with('.') {
        return Err(NameError::Edges);
    }
    if let Some(c) = part.chars().find(|c| WINDOWS_CHARS.contains(c) || c.is_control()) {
        return Err(NameError::Char(c));
    }
    if windows_reserved(part) {
        return Err(NameError::Reserved(part.to_string()));
    }
    if part.len() > MAX_PART {
        return Err(NameError::TooLong);
    }
    Ok(())
}

/// The path of the saved query named `input` (`/`-separated, relative to the scripts
/// directory), with the extension `ext` of its language: `reports/daily` → `reports/daily.sql`.
/// A typed `.sql` (any case) is kept as the extension; other dots belong to the name.
pub fn parse_name(input: &str, ext: &str) -> Result<String, NameError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(NameError::Empty);
    }
    let dotted = format!(".{ext}");
    let lower = text.to_lowercase();
    let base =
        if lower.ends_with(&dotted) && text.len() > dotted.len() { &text[..text.len() - dotted.len()] } else { text };
    let parts: Vec<&str> = base.split('/').collect();
    let last = parts.len() - 1;
    for (i, p) in parts.iter().enumerate() {
        if i == last {
            check_part(p)?;
            check_part(&format!("{p}{dotted}"))?;
        } else {
            check_part(p)?;
        }
    }
    Ok(format!("{base}{dotted}"))
}

/// A folder path typed by the user (no extension): `a/b`.
pub fn parse_folder(input: &str) -> Result<String, NameError> {
    let text = input.trim().trim_end_matches('/');
    if text.is_empty() {
        return Err(NameError::Empty);
    }
    for p in text.split('/') {
        check_part(p)?;
    }
    Ok(text.to_string())
}

/// The key two names are compared by: case folded, and Hangul written as separate jamo
/// (macOS may store names that way) composed into syllables. Two names with the same key are
/// the same file on a case-insensitive file system.
pub fn fold(s: &str) -> String {
    compose_hangul(s).to_lowercase()
}

fn compose_hangul(s: &str) -> String {
    const S_BASE: u32 = 0xAC00;
    const L_BASE: u32 = 0x1100;
    const V_BASE: u32 = 0x1161;
    const T_BASE: u32 = 0x11A7;
    let mut out: Vec<char> = Vec::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        if let Some(&last) = out.last() {
            let lp = last as u32;
            // leading consonant + vowel
            if (L_BASE..L_BASE + 19).contains(&lp)
                && (V_BASE..V_BASE + 21).contains(&cp)
                && let (Some(ch), Some(l)) =
                    (char::from_u32(S_BASE + ((lp - L_BASE) * 21 + (cp - V_BASE)) * 28), out.last_mut())
            {
                *l = ch;
                continue;
            }
            // syllable without a final + trailing consonant
            if (S_BASE..S_BASE + 11172).contains(&lp)
                && (lp - S_BASE).is_multiple_of(28)
                && (T_BASE + 1..T_BASE + 28).contains(&cp)
                && let (Some(ch), Some(l)) = (char::from_u32(lp + (cp - T_BASE)), out.last_mut())
            {
                *l = ch;
                continue;
            }
        }
        out.push(c);
    }
    out.into_iter().collect()
}
