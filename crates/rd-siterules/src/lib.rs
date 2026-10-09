//! The rules that recognise release pages, and the file they travel in (RD-110-04).
//!
//! A release page is recognised because a *rule* describes it, not because somebody built a
//! plugin for it. Every rule is a person's own: since RD-1230-03 rules carry no signature and
//! travel as exchange files ([`exchange`]) that one installation exports and another imports
//! with their switches, and the app brings only a short list of switched-off examples for
//! sites that publish free software and freely licensed media. This crate is the format and the
//! exchange file. The executor that runs a rule is RD-110-05 and lives in [`exec`].
//!
//! A leaf on purpose: `serde`, `regex`, `url` and a clock. No database, no HTTP
//! client, no captcha broker, no Wasm runtime. The database stores a user rule as JSON
//! without reading it, and the system boundary that accepts one parses it through [`Rule`]
//! before it is written. The executor ([`exec`], RD-110-05) keeps the same shape: it takes a
//! fetcher, a resolver, a captcha broker and a clock as traits from its caller, so the three
//! bolts it owns — host narrowing, redirect checking, the ban on private address ranges —
//! are testable without a network and the HTTP client stays where the application already
//! has one.

#![warn(unreachable_pub)]

pub mod catalogue;
pub mod exchange;
pub mod exec;
pub mod format;
pub mod groups;
pub mod selftest;
pub mod step;
mod text;

pub use catalogue::{Catalogue, CatalogueError};
pub use exchange::{EXCHANGE_VERSION, Exchange, ExchangeEntry, examples};
pub use exec::{
    Crawl, CrawlGroup, Executor, GroupLink, Limits, MAX_LINKS, PickEntry, PickList, RunError,
    ports::{
        CaptchaRequest, CaptchaSolver, Clock, FetchFailure, FetchRequest, FetchResponse, Fetcher,
        HostResolver, Method, SystemClock,
    },
};
pub use format::{Match, PackageSource, Rule, RuleError};
pub use groups::{GroupMirrors, Groups, Pick};
pub use selftest::{RuleReport, Verdict};
pub use step::{Decoding, Step};
