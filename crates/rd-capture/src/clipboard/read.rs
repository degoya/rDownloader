//! One look at the clipboard: the text, unless a password manager marked it as concealed
//! (RD-1200-03).
//!
//! The marks are asked for before and after the text is read, and either answer withholds it.
//! A password manager that copies a secret between the first question and the read is caught by
//! the second; one that copies over the secret between the read and the second question would
//! need two clipboard changes within the same few milliseconds.

use arboard::{Clipboard, Error as ClipboardError};

use super::marks;

/// What one look at the clipboard found.
pub(super) enum Read {
    Text(String),
    /// The clipboard holds no text: empty, or a picture.
    Nothing,
    /// A password manager marked what is on the clipboard as concealed. The text is not taken,
    /// and nothing about it is logged.
    Concealed,
    /// It could not be read this time; the next tick tries again.
    Failed,
}

/// The clipboard and the probe for its concealment marks, opened together and kept between
/// reads.
pub(super) struct Access {
    text: Clipboard,
    marks: marks::Probe,
}

/// What the blocking half of one look came to.
enum Looked {
    Text(Result<String, ClipboardError>),
    Concealed,
    /// The marks could not be asked for; carries why.
    Unmarked(anyhow::Error),
}

impl Access {
    fn concealed(&mut self) -> anyhow::Result<bool> {
        self.marks.marks().map(|found| marks::concealed(&found))
    }

    fn look(&mut self) -> Looked {
        match self.concealed() {
            Ok(true) => return Looked::Concealed,
            Ok(false) => {}
            Err(error) => return Looked::Unmarked(error),
        }
        let text = self.text.get_text();
        match self.concealed() {
            Ok(true) => Looked::Concealed,
            Ok(false) => Looked::Text(text),
            Err(error) => Looked::Unmarked(error),
        }
    }
}

/// Reads the clipboard once, opening it first when it is not open yet.
pub(super) async fn read_text(
    clipboard: &mut Option<Access>,
    unavailable_logged: &mut bool,
) -> Read {
    if clipboard.is_none() {
        match Clipboard::new() {
            Ok(text) => {
                *clipboard = Some(Access {
                    text,
                    marks: marks::Probe::new(),
                });
                *unavailable_logged = false;
            }
            Err(error) => {
                if !*unavailable_logged {
                    tracing::warn!(%error, "clipboard unavailable; retrying");
                    *unavailable_logged = true;
                }
                return Read::Failed;
            }
        }
    }
    let Some(active_clipboard) = clipboard.take() else {
        return Read::Failed;
    };
    // Off the runtime. `arboard` talks to the window server synchronously and waits for
    // whichever program owns the clipboard to answer, so a frozen browser, a remote-desktop
    // session with clipboard forwarding or a compositor under load used to take a tokio worker
    // with it -- and a desktop agent has few enough workers that this could stall the event
    // stream, the transfer poll and Click'n'Load along with it. `notify.rs` does the same for
    // its equally blocking call (RD-109-08). The marks are asked for the same way.
    //
    // The handle travels with the call and comes back: `arboard` only truly opens the Windows
    // clipboard for the length of one operation, and these operations are strictly sequential --
    // never two at once, which is the case its documentation warns about. The Windows probe
    // opens it for its own two questions and closes it again before the text is read.
    let read = tokio::task::spawn_blocking(move || {
        let mut active_clipboard = active_clipboard;
        let looked = active_clipboard.look();
        (active_clipboard, looked)
    })
    .await;
    let (returned, looked) = match read {
        Ok(pair) => pair,
        Err(error) => {
            tracing::warn!(%error, "clipboard read task ended; reopening");
            return Read::Failed;
        }
    };
    *clipboard = Some(returned);
    match looked {
        Looked::Text(Ok(text)) => {
            *unavailable_logged = false;
            Read::Text(text)
        }
        Looked::Concealed => {
            *unavailable_logged = false;
            Read::Concealed
        }
        Looked::Text(Err(ClipboardError::ContentNotAvailable)) => Read::Nothing,
        Looked::Text(Err(ClipboardError::ClipboardOccupied)) => Read::Failed,
        Looked::Text(Err(error)) => {
            if !*unavailable_logged {
                tracing::warn!(%error, "clipboard read failed; retrying");
                *unavailable_logged = true;
            }
            *clipboard = None;
            Read::Failed
        }
        // Withheld rather than read unmarked: a clipboard whose marks cannot be asked for is
        // treated like one that cannot be read at all.
        Looked::Unmarked(error) => {
            if !*unavailable_logged {
                tracing::warn!(%error, "the clipboard's concealment marks could not be read; retrying");
                *unavailable_logged = true;
            }
            Read::Failed
        }
    }
}
