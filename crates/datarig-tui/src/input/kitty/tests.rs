use super::*;

#[test]
fn requests_only_disambiguation() {
    let f = enhancement_flags();
    assert!(f.contains(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    assert!(!f.contains(KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES), "would break IME text");
    assert!(!f.contains(KeyboardEnhancementFlags::REPORT_EVENT_TYPES));
}

#[test]
fn push_and_pop_sequences() {
    // CSI > flags u  /  CSI < 1 u (kitty keyboard protocol)
    assert_eq!(push_sequence(), "\x1b[>1u");
    assert_eq!(pop_sequence(), "\x1b[<1u");
    assert_eq!(run_key_label(true), "Ctrl+Enter");
    assert_eq!(run_key_label(false), "Ctrl+E");
}
