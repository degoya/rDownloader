use std::path::Path;

use super::windows_wrapper_path;

#[test]
fn a_wrapper_path_with_a_control_character_is_refused() {
    // A line break would end the `shell.Run` statement and make the rest of the path a
    // VBScript statement of its own.
    for hostile in [
        "C:\\Tools\\rdownloader\nshell.Run Chr(34) & \"calc.exe\" & Chr(34), 0, False",
        "C:\\Tools\\rdownloader\r\nevil.cmd",
        "C:\\Tools\\rdownloader\0.cmd",
    ] {
        assert!(
            windows_wrapper_path(Path::new(hostile)).is_err(),
            "accepted {hostile:?}"
        );
    }
    assert_eq!(
        windows_wrapper_path(Path::new(r"C:\Portable 100%\O'Reilly\capture.vbs"))
            .expect("an ordinary path"),
        r"C:\Portable 100%\O'Reilly\capture.vbs"
    );
}
