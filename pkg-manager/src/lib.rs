//! Library surface for zrpkg, existing solely so `fuzz/` can link against
//! the parsing and verification code that handles untrusted, network-
//! sourced input - the archive/manifest path validation, signature
//! checking, and manifest JSON - without duplicating it. The binary
//! (`main.rs`) does not use this; it declares its own module tree over the
//! same files, which is the normal way a Rust package has both a lib and a
//! bin target.

pub mod db;
pub mod fetch;
pub mod manifest;
pub mod remove;
