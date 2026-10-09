//! The X11 half of the Linux probe: the clipboard owner is asked for its `TARGETS` through a
//! connection and a hidden window of the probe's own, and for the hint's content when it offers
//! one. `x11rb`'s pure-Rust connection, the one `arboard` and the shortcut listener link.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, GetPropertyReply, Window,
            WindowClass,
        },
    },
    rust_connection::RustConnection,
};

use super::{KDE_HINT, Mark};

/// How long the clipboard owner has to answer one question.
const ANSWER_WAIT: Duration = Duration::from_millis(500);

/// Longest answer read, in 32-bit words: a list of targets, or the hint's few bytes.
const MAX_ANSWER_WORDS: u32 = 1024;

/// Longest hint content kept; `secret` is six bytes.
const MAX_MARK_BYTES: usize = 64;

pub(super) struct Selection {
    connection: RustConnection,
    window: Window,
    clipboard: Atom,
    targets: Atom,
    incr: Atom,
    property: Atom,
    hint: Atom,
}

impl Selection {
    pub(super) fn open() -> Result<Self> {
        let (connection, screen) = x11rb::connect(None).context("connect to the X server")?;
        let root = connection
            .setup()
            .roots
            .get(screen)
            .context("the X server has no such screen")?
            .root;
        let window = connection.generate_id()?;
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &CreateWindowAux::new(),
            )?
            .check()?;
        let atom = |name: &str| -> Result<Atom> {
            Ok(connection
                .intern_atom(false, name.as_bytes())?
                .reply()?
                .atom)
        };
        let (clipboard, targets, incr, property, hint) = (
            atom("CLIPBOARD")?,
            atom("TARGETS")?,
            atom("INCR")?,
            atom("RDOWNLOADER_CAPTURE_MARKS")?,
            atom(KDE_HINT)?,
        );
        Ok(Self {
            connection,
            window,
            clipboard,
            targets,
            incr,
            property,
            hint,
        })
    }

    /// The hint, when the clipboard owner offers it; nothing when nobody owns the clipboard.
    pub(super) fn marks(&self) -> Result<Vec<Mark>> {
        let Some(answer) = self.convert(self.targets)? else {
            return Ok(Vec::new());
        };
        let offered = match answer.value32() {
            Some(mut atoms) if answer.type_ != self.incr => atoms.any(|atom| atom == self.hint),
            _ => anyhow::bail!("the clipboard owner answered TARGETS with something else"),
        };
        if !offered {
            return Ok(Vec::new());
        }
        // An answer that is missing, refused or sent in pieces leaves the content unread, which
        // the rule takes as the hint saying `secret`.
        let value = self
            .convert(self.hint)?
            .filter(|answer| answer.type_ != self.incr)
            .map(|answer| answer.value.into_iter().take(MAX_MARK_BYTES).collect());
        Ok(vec![Mark {
            format: KDE_HINT,
            value,
        }])
    }

    /// Asks the clipboard owner for `target`; `None` when nobody owns the clipboard or the owner
    /// declines.
    fn convert(&self, target: Atom) -> Result<Option<GetPropertyReply>> {
        self.connection.convert_selection(
            self.window,
            self.clipboard,
            target,
            self.property,
            x11rb::CURRENT_TIME,
        )?;
        self.connection.flush()?;
        let deadline = Instant::now() + ANSWER_WAIT;
        loop {
            match self.connection.poll_for_event()? {
                // The target is compared too, so a late answer to an earlier question that ran
                // out of time is not taken for this one.
                Some(Event::SelectionNotify(event))
                    if event.requestor == self.window && event.target == target =>
                {
                    if event.property == x11rb::NONE {
                        return Ok(None);
                    }
                    let answer = self
                        .connection
                        .get_property(
                            true,
                            self.window,
                            self.property,
                            AtomEnum::ANY,
                            0,
                            MAX_ANSWER_WORDS,
                        )?
                        .reply()?;
                    return Ok(Some(answer));
                }
                Some(_) => {}
                None if Instant::now() >= deadline => {
                    anyhow::bail!("the clipboard owner did not answer in time")
                }
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }
}
