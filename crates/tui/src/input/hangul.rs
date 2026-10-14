//! Hangul typed while a key command was meant. With a Korean input
//! source the terminal sends U+3153 HANGUL LETTER EO for the `j` key; outside text input the
//! app reads it back as the QWERTY key at the same place of the Korean 2-set (dubeolsik) layout.
//!
//! IMEs may also send composed text: a precomposed syllable (U+D558 HA), a compound vowel
//! (U+3158 WA) or a compound final consonant (U+3133 KIYEOK-SIOS). Those are split into their
//! jamo and returned as the key sequence that typed them (HA -> `g k`, WA -> `h k`,
//! U+B2ED DALG -> `e k f r`).
//!
//! Limits: keys whose Shift variant gives the same jamo (`G`, `J`, ...) cannot be told apart,
//! and an IME may hold keys back until it finishes a syllable. The status bar shows the
//! Hangul/Latin indicator (`status.hangul`) when this mapping is used so the user knows the
//! input source is Korean.

/// Jamo on each key of the 2-set layout (Shift only changes `QWERTOP`).
const KEYS: [(char, char); 33] = [
    ('q', '\u{3142}'), // U+3142 PIEUP
    ('w', '\u{3148}'), // U+3148 CIEUC
    ('e', '\u{3137}'), // U+3137 TIKEUT
    ('r', '\u{3131}'), // U+3131 KIYEOK
    ('t', '\u{3145}'), // U+3145 SIOS
    ('y', '\u{315B}'), // U+315B YO
    ('u', '\u{3155}'), // U+3155 YEO
    ('i', '\u{3151}'), // U+3151 YA
    ('o', '\u{3150}'), // U+3150 AE
    ('p', '\u{3154}'), // U+3154 E
    ('a', '\u{3141}'), // U+3141 MIEUM
    ('s', '\u{3134}'), // U+3134 NIEUN
    ('d', '\u{3147}'), // U+3147 IEUNG
    ('f', '\u{3139}'), // U+3139 RIEUL
    ('g', '\u{314E}'), // U+314E HIEUH
    ('h', '\u{3157}'), // U+3157 O
    ('j', '\u{3153}'), // U+3153 EO
    ('k', '\u{314F}'), // U+314F A
    ('l', '\u{3163}'), // U+3163 I
    ('z', '\u{314B}'), // U+314B KHIEUKH
    ('x', '\u{314C}'), // U+314C THIEUTH
    ('c', '\u{314A}'), // U+314A CHIEUCH
    ('v', '\u{314D}'), // U+314D PHIEUPH
    ('b', '\u{3160}'), // U+3160 YU
    ('n', '\u{315C}'), // U+315C U
    ('m', '\u{3161}'), // U+3161 EU
    ('Q', '\u{3143}'), // U+3143 SSANGPIEUP
    ('W', '\u{3149}'), // U+3149 SSANGCIEUC
    ('E', '\u{3138}'), // U+3138 SSANGTIKEUT
    ('R', '\u{3132}'), // U+3132 SSANGKIYEOK
    ('T', '\u{3146}'), // U+3146 SSANGSIOS
    ('O', '\u{3152}'), // U+3152 YAE
    ('P', '\u{3156}'), // U+3156 YE
];

/// Compound jamo typed as two keys: final consonant clusters and compound vowels.
const COMPOUND: [(char, [char; 2]); 18] = [
    ('\u{3133}', ['\u{3131}', '\u{3145}']), // U+3133 KIYEOK-SIOS, U+3131 KIYEOK, U+3145 SIOS
    ('\u{3135}', ['\u{3134}', '\u{3148}']), // U+3135 NIEUN-CIEUC, U+3134 NIEUN, U+3148 CIEUC
    ('\u{3136}', ['\u{3134}', '\u{314E}']), // U+3136 NIEUN-HIEUH, U+3134 NIEUN, U+314E HIEUH
    ('\u{313A}', ['\u{3139}', '\u{3131}']), // U+313A RIEUL-KIYEOK, U+3139 RIEUL, U+3131 KIYEOK
    ('\u{313B}', ['\u{3139}', '\u{3141}']), // U+313B RIEUL-MIEUM, U+3139 RIEUL, U+3141 MIEUM
    ('\u{313C}', ['\u{3139}', '\u{3142}']), // U+313C RIEUL-PIEUP, U+3139 RIEUL, U+3142 PIEUP
    ('\u{313D}', ['\u{3139}', '\u{3145}']), // U+313D RIEUL-SIOS, U+3139 RIEUL, U+3145 SIOS
    ('\u{313E}', ['\u{3139}', '\u{314C}']), // U+313E RIEUL-THIEUTH, U+3139 RIEUL, U+314C THIEUTH
    ('\u{313F}', ['\u{3139}', '\u{314D}']), // U+313F RIEUL-PHIEUPH, U+3139 RIEUL, U+314D PHIEUPH
    ('\u{3140}', ['\u{3139}', '\u{314E}']), // U+3140 RIEUL-HIEUH, U+3139 RIEUL, U+314E HIEUH
    ('\u{3144}', ['\u{3142}', '\u{3145}']), // U+3144 PIEUP-SIOS, U+3142 PIEUP, U+3145 SIOS
    ('\u{3158}', ['\u{3157}', '\u{314F}']), // U+3158 WA, U+3157 O, U+314F A
    ('\u{3159}', ['\u{3157}', '\u{3150}']), // U+3159 WAE, U+3157 O, U+3150 AE
    ('\u{315A}', ['\u{3157}', '\u{3163}']), // U+315A OE, U+3157 O, U+3163 I
    ('\u{315D}', ['\u{315C}', '\u{3153}']), // U+315D WEO, U+315C U, U+3153 EO
    ('\u{315E}', ['\u{315C}', '\u{3154}']), // U+315E WE, U+315C U, U+3154 E
    ('\u{315F}', ['\u{315C}', '\u{3163}']), // U+315F WI, U+315C U, U+3163 I
    ('\u{3162}', ['\u{3161}', '\u{3163}']), // U+3162 YI, U+3161 EU, U+3163 I
];

/// Initial consonants of a precomposed syllable, in Unicode order.
const INITIALS: [char; 19] = [
    '\u{3131}', // U+3131 KIYEOK
    '\u{3132}', // U+3132 SSANGKIYEOK
    '\u{3134}', // U+3134 NIEUN
    '\u{3137}', // U+3137 TIKEUT
    '\u{3138}', // U+3138 SSANGTIKEUT
    '\u{3139}', // U+3139 RIEUL
    '\u{3141}', // U+3141 MIEUM
    '\u{3142}', // U+3142 PIEUP
    '\u{3143}', // U+3143 SSANGPIEUP
    '\u{3145}', // U+3145 SIOS
    '\u{3146}', // U+3146 SSANGSIOS
    '\u{3147}', // U+3147 IEUNG
    '\u{3148}', // U+3148 CIEUC
    '\u{3149}', // U+3149 SSANGCIEUC
    '\u{314A}', // U+314A CHIEUCH
    '\u{314B}', // U+314B KHIEUKH
    '\u{314C}', // U+314C THIEUTH
    '\u{314D}', // U+314D PHIEUPH
    '\u{314E}', // U+314E HIEUH
];

/// Final consonants of a precomposed syllable (index 0 = none), in Unicode order.
const FINALS: [Option<char>; 28] = [
    None,
    Some('\u{3131}'), // U+3131 KIYEOK
    Some('\u{3132}'), // U+3132 SSANGKIYEOK
    Some('\u{3133}'), // U+3133 KIYEOK-SIOS
    Some('\u{3134}'), // U+3134 NIEUN
    Some('\u{3135}'), // U+3135 NIEUN-CIEUC
    Some('\u{3136}'), // U+3136 NIEUN-HIEUH
    Some('\u{3137}'), // U+3137 TIKEUT
    Some('\u{3139}'), // U+3139 RIEUL
    Some('\u{313A}'), // U+313A RIEUL-KIYEOK
    Some('\u{313B}'), // U+313B RIEUL-MIEUM
    Some('\u{313C}'), // U+313C RIEUL-PIEUP
    Some('\u{313D}'), // U+313D RIEUL-SIOS
    Some('\u{313E}'), // U+313E RIEUL-THIEUTH
    Some('\u{313F}'), // U+313F RIEUL-PHIEUPH
    Some('\u{3140}'), // U+3140 RIEUL-HIEUH
    Some('\u{3141}'), // U+3141 MIEUM
    Some('\u{3142}'), // U+3142 PIEUP
    Some('\u{3144}'), // U+3144 PIEUP-SIOS
    Some('\u{3145}'), // U+3145 SIOS
    Some('\u{3146}'), // U+3146 SSANGSIOS
    Some('\u{3147}'), // U+3147 IEUNG
    Some('\u{3148}'), // U+3148 CIEUC
    Some('\u{314A}'), // U+314A CHIEUCH
    Some('\u{314B}'), // U+314B KHIEUKH
    Some('\u{314C}'), // U+314C THIEUTH
    Some('\u{314D}'), // U+314D PHIEUPH
    Some('\u{314E}'), // U+314E HIEUH
];

const SYLLABLE_FIRST: u32 = 0xAC00;
const SYLLABLE_LAST: u32 = 0xD7A3;
/// First vowel of the compatibility jamo block (U+314F HANGUL LETTER A); the 21 vowels follow in syllable order.
const VOWEL_FIRST: u32 = 0x314F;

fn push_jamo(j: char, out: &mut Vec<char>) {
    if let Some((_, parts)) = COMPOUND.iter().find(|(c, _)| *c == j) {
        for p in parts {
            push_jamo(*p, out);
        }
    } else if let Some((k, _)) = KEYS.iter().find(|(_, c)| *c == j) {
        out.push(*k);
    }
}

/// The QWERTY keys that type `c` on the 2-set layout, in order; `None` when `c` is not a
/// compatibility jamo (U+3131–U+3163) or a precomposed syllable (U+AC00–U+D7A3).
pub fn keys(c: char) -> Option<Vec<char>> {
    let mut out = Vec::new();
    let u = c as u32;
    if (SYLLABLE_FIRST..=SYLLABLE_LAST).contains(&u) {
        let i = u - SYLLABLE_FIRST;
        let (l, v, t) = (i / (21 * 28), (i / 28) % 21, i % 28);
        push_jamo(INITIALS[l as usize], &mut out);
        push_jamo(char::from_u32(VOWEL_FIRST + v)?, &mut out);
        if let Some(f) = FINALS[t as usize] {
            push_jamo(f, &mut out);
        }
    } else {
        push_jamo(c, &mut out);
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests;
