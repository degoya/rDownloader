use super::*;

#[test]
fn the_maintenance_sentence_alone_is_no_notes() {
    assert_eq!(index_notes(format!("{MAINTENANCE}\n")), None);
    assert_eq!(index_notes(format!("  {MAINTENANCE}")), None);
}

#[test]
fn other_notes_are_kept_as_written() {
    let notes = "In 0.1.13: Captcha pages are solved again.\n".to_owned();
    assert_eq!(index_notes(notes.clone()), Some(notes));
    let longer = format!("{MAINTENANCE} Links to folders are recognised again.");
    assert_eq!(index_notes(longer.clone()), Some(longer));
}
