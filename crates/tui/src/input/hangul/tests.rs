// Hangul jamo and syllables are written as `\u{…}` escapes (the repository keeps Korean text
// in locales/ko.toml only); the comments name the QWERTY keys of the 2-Set layout instead.

use super::*;

fn key_str(c: char) -> String {
    keys(c).map(|k| k.into_iter().collect()).unwrap_or_default()
}

fn jamo_of_key(k: char) -> char {
    KEYS.iter().find(|(q, _)| *q == k).map(|(_, j)| *j).expect("a 2-set key")
}

fn is_vowel(j: char) -> bool {
    (VOWEL_FIRST..VOWEL_FIRST + 21).contains(&(j as u32))
}

fn combine(a: char, b: char) -> Option<char> {
    COMPOUND.iter().find(|(_, p)| *p == [a, b]).map(|(c, _)| *c)
}

fn syllable(l: char, v: char, t: Option<char>) -> char {
    let li = INITIALS.iter().position(|c| *c == l).unwrap() as u32;
    let vi = v as u32 - VOWEL_FIRST;
    let ti = FINALS.iter().position(|c| *c == t).unwrap() as u32;
    char::from_u32(SYLLABLE_FIRST + (li * 21 + vi) * 28 + ti).unwrap()
}

/// A 2-set IME: composes the jamo of `keys` into syllables, including compound vowels, compound
/// finals and a final consonant moving to the next syllable when a vowel follows.
fn compose(keys: &[char]) -> String {
    let mut out = String::new();
    let (mut l, mut v, mut t): (Option<char>, Option<char>, Option<char>) = (None, None, None);
    let flush =
        |out: &mut String, l: &mut Option<char>, v: &mut Option<char>, t: &mut Option<char>| match (l.take(), v.take())
        {
            (Some(a), Some(b)) => out.push(syllable(a, b, t.take())),
            (a, b) => out.extend(a.into_iter().chain(b).chain(t.take())),
        };
    for j in keys.iter().map(|k| jamo_of_key(*k)) {
        if is_vowel(j) {
            match (l, v, t) {
                (Some(_), None, _) => v = Some(j),
                (_, Some(a), None) if combine(a, j).is_some() => v = combine(a, j),
                (Some(_), Some(_), Some(f)) => {
                    // The final (or the last part of a compound final) starts the next syllable.
                    let (keep, moved) = match COMPOUND.iter().find(|(c, _)| *c == f) {
                        Some((_, [a, b])) => (Some(*a), *b),
                        None => (None, f),
                    };
                    t = keep;
                    flush(&mut out, &mut l, &mut v, &mut t);
                    l = Some(moved);
                    v = Some(j);
                }
                _ => {
                    flush(&mut out, &mut l, &mut v, &mut t);
                    v = Some(j);
                }
            }
        } else {
            match (l, v, t) {
                (Some(_), Some(_), None) if FINALS.contains(&Some(j)) => t = Some(j),
                (Some(_), Some(_), Some(f)) if combine(f, j).is_some() => t = combine(f, j),
                _ => {
                    flush(&mut out, &mut l, &mut v, &mut t);
                    l = Some(j);
                }
            }
        }
    }
    flush(&mut out, &mut l, &mut v, &mut t);
    out
}

#[test]
fn all_51_compatibility_jamo() {
    let (mut consonants, mut vowels) = (Vec::new(), Vec::new());
    for u in 0x3131..=0x3163u32 {
        let c = char::from_u32(u).unwrap();
        let k = keys(c).unwrap_or_else(|| panic!("{c} has no keys"));
        assert!(k.iter().all(|q| q.is_ascii_alphabetic()), "{c}: {k:?}");
        if is_vowel(c) { &mut vowels } else { &mut consonants }.push((c, k.len()));
    }
    assert_eq!(consonants.len(), 30);
    assert_eq!(vowels.len(), 21);
    let single = |v: &[(char, usize)]| v.iter().filter(|(_, n)| *n == 1).count();
    assert_eq!(single(&consonants), 19, "14 basic + 5 double consonants have their own key");
    assert_eq!(consonants.len() - single(&consonants), 11, "compound finals take two keys");
    assert_eq!(single(&vowels), 14);
    assert_eq!(vowels.len() - single(&vowels), 7, "compound vowels take two keys");
    // Spot checks of single keys and of the compound rules.
    for (c, k) in [
        ('\u{3153}', "j"),
        ('\u{314F}', "k"),
        ('\u{3157}', "h"),
        ('\u{3163}', "l"),
        ('\u{314E}', "g"),
        ('\u{3158}', "hk"),
        ('\u{3133}', "rt"),
    ] {
        assert_eq!(key_str(c), k, "{c}");
    }
    for (c, k) in [('\u{3162}', "ml"), ('\u{3140}', "fg"), ('\u{3144}', "qt"), ('\u{315E}', "np")] {
        assert_eq!(key_str(c), k, "{c}");
    }
}

#[test]
fn shift_jamo_are_uppercase_keys() {
    let shifted: String = "\u{3143}\u{3149}\u{3138}\u{3132}\u{3146}\u{3152}\u{3156}".chars().map(key_str).collect();
    assert_eq!(shifted, "QWERTOP");
}

#[test]
fn not_hangul_has_no_keys() {
    for c in ['a', 'J', '1', ' ', '?', 'ア', '中', '\u{1100}', '\u{3130}', '\u{3164}', '\u{D7A4}'] {
        assert_eq!(keys(c), None, "{c:?}");
    }
}

#[test]
fn syllables_split_into_their_keys() {
    for (c, k) in [
        ('\u{D558}', "gk"),
        ('\u{B2ED}', "ekfr"),
        ('\u{AD1C}', "rhos"),
        ('\u{BDC1}', "qnpfr"),
        ('\u{AC00}', "rk"),
        ('\u{D7A3}', "glg"),
    ] {
        assert_eq!(key_str(c), k, "{c}");
    }
}

#[test]
fn every_syllable_round_trips_through_a_2set_ime() {
    for u in SYLLABLE_FIRST..=SYLLABLE_LAST {
        let c = char::from_u32(u).unwrap();
        let k = keys(c).unwrap();
        assert!((2..=5).contains(&k.len()), "{c}: {k:?}");
        assert_eq!(compose(&k), c.to_string(), "{c} typed as {k:?}");
    }
    // The IME model itself: a final consonant moves on when a vowel follows.
    assert_eq!(compose(&['g', 'k', 's', 'r', 'n', 'f']), "\u{D55C}\u{AD74}");
    assert_eq!(compose(&['e', 'k', 'f', 'r', 'k']), "\u{B2EC}\u{AC00}");
}
