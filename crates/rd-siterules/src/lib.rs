//! The rules that recognise release pages, and the signed pack they travel in (RD-110-04).
//!
//! A release page is recognised because a *rule* describes it, not because somebody built a
//! plugin for it. The project's rules are one signed document under their own trust root
//! (`rd_sign::Role::SiteRules`), so they are as trustworthy as a plugin or the tool manifest.
//! Since RD-130-07 that document is a release artifact rather than part of the binary: an
//! installation starts with no rule at all, and importing the file verifies it and stores its
//! rules in the database as the person's own, switched off. This crate is the format and the
//! carrier. The executor that runs a rule is
//! RD-110-05 and lives in [`exec`]; the rules themselves are RD-110-10 onwards.
//!
//! A leaf on purpose: `rd-sign`, `serde`, `regex`, `url` and a clock. No database, no HTTP
//! client, no captcha broker, no Wasm runtime. The database stores a user rule as JSON
//! without reading it, and the system boundary that accepts one parses it through [`Rule`]
//! before it is written. The executor ([`exec`], RD-110-05) keeps the same shape: it takes a
//! fetcher, a resolver, a captcha broker and a clock as traits from its caller, so the three
//! bolts it owns — host narrowing, redirect checking, the ban on private address ranges —
//! are testable without a network and the HTTP client stays where the application already
//! has one.

pub mod catalogue;
pub mod exec;
pub mod format;
pub mod pack;
pub mod selftest;
pub mod step;
mod text;

pub use catalogue::{Catalogue, CatalogueError};
pub use exec::{
    Crawl, Executor, Limits, MAX_LINKS, RunError,
    ports::{
        CaptchaRequest, CaptchaSolver, Clock, FetchFailure, FetchRequest, FetchResponse, Fetcher,
        HostResolver, Method, SystemClock,
    },
};
pub use format::{Match, PackageSource, Rule, RuleError};
pub use pack::{FORMAT_VERSION, PackError, RulePack, SITE_RULES_DOMAIN, sign, verify, verify_with};
pub use selftest::{RuleReport, Verdict};
pub use step::{Decoding, Step};
