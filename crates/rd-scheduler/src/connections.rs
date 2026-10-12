//! The connections the runners hold right now (RD-1270-21).
//!
//! A Usenet package used to show only how many of its files were running - "2 active" - while
//! the two files together kept all ten of the provider's connections busy. The runner that
//! owns a connection pool answers with what its connections are doing; the queue only asks
//! and hands the answer on. Nothing here is stored: it is a reading of the moment.

use std::collections::HashMap;

use rd_core::{DownloadId, UsenetServerId};

use crate::{ExternalRunner, SchedulerHandle};

/// What a transport's connections are doing right now.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LiveConnections {
    /// Connections working for each running file. A file waiting for a free connection has
    /// no entry; one connection shared by two files is split between them, so the files
    /// together never hold more than the servers have busy.
    pub files: HashMap<DownloadId, u32>,
    /// Every server of the transport's pool, in the pool's order.
    pub servers: Vec<ServerConnections>,
}

/// One server's connections: those carrying a request, and the most the pool may open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerConnections {
    pub server: UsenetServerId,
    pub busy: u32,
    /// The server's own limit, lowered by the per-file setting when that is smaller.
    pub limit: u32,
}

impl LiveConnections {
    /// `other` added to this reading; a transport reports its own files and servers only.
    fn merge(&mut self, other: Self) {
        self.files.extend(other.files);
        self.servers.extend(other.servers);
    }
}

impl SchedulerHandle {
    /// What the connections of every runner that keeps a pool are doing right now.
    #[must_use]
    pub fn live_connections(&self) -> LiveConnections {
        let mut reading = LiveConnections::default();
        for runner in self.runners.all() {
            if let Some(connections) = ExternalRunner::live_connections(runner.as_ref()) {
                reading.merge(connections);
            }
        }
        reading
    }
}
