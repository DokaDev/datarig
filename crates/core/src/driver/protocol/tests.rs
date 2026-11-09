use super::*;

#[test]
fn a_position_counts_characters_from_one() {
    let sql = "SELECT 1 +";
    assert_eq!(ServerMessage::byte_offset(sql, 1), Some(0));
    assert_eq!(ServerMessage::byte_offset(sql, 10), Some(9));
    assert_eq!(&sql[ServerMessage::byte_offset(sql, 8).unwrap()..], "1 +");
}

#[test]
fn a_position_in_multibyte_text_lands_on_a_character_boundary() {
    // Two, three and four bytes before the name.
    let sql = "SELECT 'é€𝄞' FROM t";
    let at = ServerMessage::byte_offset(sql, 19).expect("in the text");
    assert_eq!(at, "SELECT 'é€𝄞' FROM ".len());
    assert_eq!(&sql[at..], "t");
    assert_eq!(ServerMessage::byte_offset(sql, 10), Some("SELECT 'é".len()));
}

#[test]
fn a_position_outside_the_text_is_none() {
    assert_eq!(ServerMessage::byte_offset("SELECT 1", 0), None);
    assert_eq!(ServerMessage::byte_offset("SELECT 1", 9), None);
    assert_eq!(ServerMessage::byte_offset("é", 2), None);
    assert_eq!(ServerMessage::byte_offset("", 1), None);
    assert_eq!(ServerMessage::byte_offset("SELECT 1", u32::MAX), None);
}

#[test]
fn a_server_message_reads_and_prints_as_its_text() {
    let plain = DbError::Server("ERROR: division by zero".into());
    assert_eq!(format!("{plain:?}"), "Server(\"ERROR: division by zero\")");
    let DbError::Server(m) = &plain else { unreachable!() };
    assert!(m.contains("division"));
    assert_eq!(m.to_string(), "ERROR: division by zero");
    let pointed = ServerMessage { text: "ERROR: x".into(), position: Some(3), hint: Some("h".into()) };
    assert_eq!(DbError::Server(pointed.clone()).raw(), "ERROR: x");
    assert!(format!("{pointed:?}").contains("position: Some(3)"));
}
