use std::path::Path;

use super::Target;
use super::run_value::{capture_beside, render, windows_command_path};

#[test]
fn a_path_with_a_quote_or_a_control_character_is_refused() {
    // A quote would close the program token and make the rest of the path arguments of their
    // own; a line break has no place in a registry command line.
    for hostile in [
        "C:\\Tools\\rdownloader\" calc.exe \"x.exe",
        "C:\\Tools\\rdownloader\nevil.exe",
        "C:\\Tools\\rdownloader\r\nevil.exe",
        "C:\\Tools\\rdownloader\0.exe",
    ] {
        assert!(
            windows_command_path(Path::new(hostile)).is_err(),
            "accepted {hostile:?}"
        );
        assert!(
            render(Target::Capture, Path::new(hostile)).is_err(),
            "rendered {hostile:?}"
        );
    }
    assert_eq!(
        windows_command_path(Path::new(
            r"C:\Portable 100%\O'Reilly\rdownloader-capture.exe"
        ))
        .expect("an ordinary path"),
        r"C:\Portable 100%\O'Reilly\rdownloader-capture.exe"
    );
}

/// RD-1400-07: the `Run` value starts the agent itself, quoted, with the flag that makes it
/// enter its working folder and write its logs; no `wscript.exe`, no `.vbs`, no `.cmd`.
#[test]
fn the_capture_entry_starts_the_agent_directly() {
    let value = render(
        Target::Capture,
        Path::new(
            r"C:\Users\O'Reilly\AppData\Local\Programs\rDownloader 100%\rdownloader-capture.exe",
        ),
    )
    .expect("a Run value");
    assert_eq!(
        value,
        r#""C:\Users\O'Reilly\AppData\Local\Programs\rDownloader 100%\rdownloader-capture.exe" run --autostart"#
    );
    for chain in ["wscript", ".vbs", ".cmd", "reg.exe"] {
        assert!(!value.contains(chain), "{value}");
    }
}

/// The service is a console program: its entry starts the agent beside it, which starts the
/// service without a window and ends.
#[test]
fn the_service_entry_starts_it_through_the_agent_beside_it() {
    let service = Path::new(r"C:\Program Files\rDownloader\rdownloader.exe");
    assert_eq!(
        capture_beside(service),
        Path::new(r"C:\Program Files\rDownloader\rdownloader-capture.exe")
    );
    assert_eq!(
        render(Target::Server, service).expect("a Run value"),
        r#""C:\Program Files\rDownloader\rdownloader-capture.exe" start-service"#
    );
    // Scoop's junction stays the junction.
    assert_eq!(
        render(
            Target::Server,
            Path::new(r"C:\Users\me\scoop\apps\rdownloader\current\rdownloader.exe")
        )
        .expect("a Run value"),
        r#""C:\Users\me\scoop\apps\rdownloader\current\rdownloader-capture.exe" start-service"#
    );
}
