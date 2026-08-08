//! Library surface for zrpkg, existing so `fuzz/` and `vakt-update` can link
//! against the parsing/verification code without duplicating it. `main.rs`
//! declares its own module tree over the same files.

pub mod config;
pub mod db;
pub mod fetch;
pub mod manifest;
pub mod remove;
