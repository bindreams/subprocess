use super::{parse_starttime_jiffies, parse_state};

// After the comm field's LAST ')', the whitespace-split tail starts at field 3
// (state, index 0); starttime is field 22 (index 19).
#[test]
fn parses_simple_stat() {
    let stat = b"1234 (my proc) S 1 1234 1234 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 8675309 0 0";
    assert_eq!(parse_starttime_jiffies(stat), Some(8675309));
    assert_eq!(parse_state(stat), Some(b'S'));
}

#[test]
fn handles_comm_containing_parens_and_spaces() {
    // comm = "a) b" — embedded ')' and space; rposition(')') must find the LAST one.
    let stat = b"7 (a) b) R 0 7 7 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 55 0 0";
    assert_eq!(parse_starttime_jiffies(stat), Some(55));
    assert_eq!(parse_state(stat), Some(b'R'));
}

#[test]
fn detects_zombie_state() {
    let stat = b"9 (gone) Z 1 9 9 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 4242 0 0";
    assert_eq!(parse_state(stat), Some(b'Z'));
}

#[test]
fn rejects_truncated_stat() {
    let stat = b"1 (init) S 0 1 1"; // far fewer than 22 fields
    assert_eq!(parse_starttime_jiffies(stat), None);
    assert_eq!(parse_state(stat), Some(b'S')); // state still readable here
}

#[test]
fn rejects_missing_close_paren() {
    let stat = b"1 (init S 0 1";
    assert_eq!(parse_starttime_jiffies(stat), None);
    assert_eq!(parse_state(stat), None);
}
