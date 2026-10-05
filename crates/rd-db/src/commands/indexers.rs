//! The commands of `writer/indexers.rs`.

use super::Reply;

/// The commands `Writer::handle_indexers` applies.
// The variant names were the flat enum's; within one area they share a postfix.
#[allow(clippy::enum_variant_names)]
pub(crate) enum IndexersCommand {
    /// Newznab indexers defined once (RD-180-19).
    CreateIndexer {
        input: Box<crate::NewIndexer>,
        reply: Reply<rd_core::Indexer>,
    },
    /// Replies with the indexer and the key reference the edit replaced.
    UpdateIndexer {
        id: rd_core::IndexerId,
        input: Box<crate::NewIndexer>,
        reply: Reply<(rd_core::Indexer, Option<String>)>,
    },
    DeleteIndexer {
        id: rd_core::IndexerId,
        reply: Reply<Option<String>>,
    },
}
