//! A move and an add of one torrent exclude each other.
//!
//! The runner checks for a running move and then adds the torrent several awaits later. A move
//! that began in between would take the torrent out of the session and empty its folder while
//! the runner added it back there (RD-1100-10). Both therefore hold a gate for the torrent: the
//! runner from its check until the add has returned, the move from its start to its end, and
//! neither begins while the other holds one.

use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard, PoisonError},
};

use rd_core::DownloadId;

/// What holds one torrent's gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Gate {
    /// That many adds are under way.
    Adding(usize),
    /// Its files are being moved.
    Relocating,
}

/// The gates of every torrent that is being added or moved in this process.
#[derive(Default)]
pub(crate) struct Gates(Mutex<HashMap<DownloadId, Gate>>);

impl Gates {
    fn lock(&self) -> MutexGuard<'_, HashMap<DownloadId, Gate>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a move of this torrent's files runs right now.
    pub(crate) fn is_relocating(&self, id: DownloadId) -> bool {
        self.lock().get(&id) == Some(&Gate::Relocating)
    }

    /// Holds the gate for an add, or `None` while a move runs. The gate opens again when the
    /// returned claim is dropped.
    pub(crate) fn begin_add(&self, id: DownloadId) -> Option<AddClaim<'_>> {
        let mut gates = self.lock();
        let adds = match gates.get(&id) {
            Some(Gate::Relocating) => return None,
            Some(Gate::Adding(adds)) => adds + 1,
            None => 1,
        };
        gates.insert(id, Gate::Adding(adds));
        Some(AddClaim { gates: self, id })
    }

    /// Holds the gate for a move; refused while an add or another move holds it.
    pub(crate) fn begin_relocation(&self, id: DownloadId) -> Result<(), &'static str> {
        let mut gates = self.lock();
        match gates.get(&id) {
            Some(Gate::Relocating) => Err("a move of this torrent is under way already"),
            Some(Gate::Adding(_)) => Err("the torrent is being added; try the move again shortly"),
            None => {
                gates.insert(id, Gate::Relocating);
                Ok(())
            }
        }
    }

    /// Opens the gate a move held.
    pub(crate) fn end_relocation(&self, id: DownloadId) {
        let mut gates = self.lock();
        if gates.get(&id) == Some(&Gate::Relocating) {
            gates.remove(&id);
        }
    }
}

/// An add's hold on a torrent's gate.
pub(crate) struct AddClaim<'a> {
    gates: &'a Gates,
    id: DownloadId,
}

impl Drop for AddClaim<'_> {
    fn drop(&mut self) {
        let mut gates = self.gates.lock();
        match gates.get(&self.id) {
            Some(Gate::Adding(adds)) if *adds > 1 => {
                let adds = adds - 1;
                gates.insert(self.id, Gate::Adding(adds));
            }
            Some(Gate::Adding(_)) => {
                gates.remove(&self.id);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use rd_core::DownloadId;

    use super::Gates;

    #[test]
    fn a_move_is_refused_while_an_add_holds_the_gate() {
        let gates = Gates::default();
        let id = DownloadId::new();
        let add = gates.begin_add(id).expect("add");
        assert!(gates.begin_relocation(id).is_err());
        assert!(!gates.is_relocating(id));
        drop(add);
        assert!(gates.begin_relocation(id).is_ok());
        assert!(gates.is_relocating(id));
    }

    #[test]
    fn an_add_is_refused_while_a_move_holds_the_gate() {
        let gates = Gates::default();
        let id = DownloadId::new();
        gates.begin_relocation(id).expect("move");
        assert!(gates.begin_add(id).is_none());
        assert!(gates.begin_relocation(id).is_err());
        gates.end_relocation(id);
        assert!(gates.begin_add(id).is_some());
    }

    #[test]
    fn the_gate_opens_only_after_the_last_add() {
        let gates = Gates::default();
        let id = DownloadId::new();
        let first = gates.begin_add(id).expect("first");
        let second = gates.begin_add(id).expect("second");
        drop(first);
        assert!(gates.begin_relocation(id).is_err());
        drop(second);
        assert!(gates.begin_relocation(id).is_ok());
    }

    #[test]
    fn other_torrents_are_not_held() {
        let gates = Gates::default();
        let _add = gates.begin_add(DownloadId::new()).expect("add");
        assert!(gates.begin_relocation(DownloadId::new()).is_ok());
    }
}
