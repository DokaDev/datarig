//! Profile colors: one of 12 names or `#rrggbb`, or an automatic color
//! picked from the profile name. Only the model lives here; the TUI maps names to RGB.

use std::fmt;

/// The 12 named colors, in palette order (the automatic color indexes this list).
pub const NAMES: [&str; 12] =
    ["red", "orange", "yellow", "green", "teal", "cyan", "blue", "indigo", "purple", "pink", "brown", "gray"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProfileColor {
    /// Index into [`NAMES`].
    Named(u8),
    Hex(u8, u8, u8),
}

impl ProfileColor {
    /// A config value: a name of [`NAMES`] (case-insensitive) or `#rrggbb`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some(hex) = s.strip_prefix('#') {
            if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let v = u32::from_str_radix(hex, 16).ok()?;
            return Some(ProfileColor::Hex((v >> 16) as u8, (v >> 8) as u8, v as u8));
        }
        NAMES.iter().position(|n| n.eq_ignore_ascii_case(s)).map(|i| ProfileColor::Named(i as u8))
    }

    /// The color of a profile without one: FNV-1a of the name, modulo the 12 names. Stable
    /// across runs and platforms.
    pub fn auto(name: &str) -> Self {
        let mut h: u32 = 0x811c_9dc5;
        for b in name.as_bytes() {
            h ^= u32::from(*b);
            h = h.wrapping_mul(0x0100_0193);
        }
        ProfileColor::Named((h % NAMES.len() as u32) as u8)
    }
}

impl fmt::Display for ProfileColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileColor::Named(i) => f.write_str(NAMES[*i as usize]),
            ProfileColor::Hex(r, g, b) => write!(f, "#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

#[cfg(test)]
mod tests;
