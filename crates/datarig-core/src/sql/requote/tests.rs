use super::*;
use crate::sql::dialect::MySqlMode;

const ESC: Dialect =
    Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false });
const NBE: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: true, dollar_quotes: false });
const ANSI: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: true, no_backslash_escapes: true, dollar_quotes: false });

/// Every string goes over as the same value; names, comments, client commands and strings
/// without escapes stay as they are.
#[test]
fn strings_are_written_as_the_other_mode_reads_them() {
    let text = "DELIMITER ;;\nCREATE TABLE `a\\b` (`v` varchar(20) DEFAULT 'a\\\\b''c\\'d', \
                w int DEFAULT (_latin1'x\\ny'), x blob DEFAULT (X'5C'), y bit(8) DEFAULT b'1', \
                z varchar(9) DEFAULT N'\\0', q varchar(9) DEFAULT \"it\\\"s\") -- 'a\\b'\n;;\nDELIMITER ;\n";
    let to_nbe = requote_strings(text, ESC, NBE).unwrap();
    assert_eq!(
        to_nbe,
        "DELIMITER ;;\nCREATE TABLE `a\\b` (`v` varchar(20) DEFAULT 'a\\b''c''d', \
         w int DEFAULT (_latin1'x\ny'), x blob DEFAULT (X'5C'), y bit(8) DEFAULT b'1', \
         z varchar(9) DEFAULT N'\0', q varchar(9) DEFAULT 'it\"s') -- 'a\\b'\n;;\nDELIMITER ;\n"
    );
    // And back: the same values, written with escapes.
    let back = requote_strings(&to_nbe, NBE, ESC).unwrap();
    assert!(back.contains("DEFAULT 'a\\\\b''c''d'"), "{back}");
    assert!(back.contains("N'\\0'") && back.contains("'it\"s'"), "{back}");
    // `\%` and `\_` are two characters each.
    assert_eq!(requote_strings("LIKE 'a\\%b\\_'", ESC, NBE).unwrap(), "LIKE 'a\\%b\\_'");
    assert_eq!(requote_strings("LIKE 'a\\%b'", NBE, ESC).unwrap(), "LIKE 'a\\\\%b'");
}

#[test]
fn the_same_mode_changes_nothing() {
    let text = "SELECT 'a\\\\b', \"x\"";
    assert_eq!(requote_strings(text, ESC, ESC).unwrap(), text);
    assert_eq!(requote_strings(text, NBE, NBE).unwrap(), text);
}

/// What cannot be requoted is said, never guessed.
#[test]
fn what_cannot_be_requoted_is_refused() {
    assert_eq!(requote_strings("SELECT \"x\"", ESC, ANSI), Err(RequoteError::DoubleQuotes));
    assert_eq!(requote_strings("SELECT 'x", ESC, NBE), Err(RequoteError::Unclosed));
    assert_eq!(requote_strings("SELECT 1", Dialect::Postgres, NBE), Err(RequoteError::NotMySql));
    // Without double quotes the ANSI difference does not matter.
    assert_eq!(requote_strings("SELECT 'a\\\\b'", ESC, ANSI).unwrap(), "SELECT 'a\\b'");
}
