//! Library surface for zrpkg, existing so `fuzz/` and `vakt-update` can link
//! against the parsing and verification code that handles untrusted,
//! network-sourced input - the archive/manifest path validation, signature
//! checking, manifest JSON, and repository URL resolution - without
//! duplicating it. The binary (`main.rs`) does not use this; it declares its
//! own module tree over the same files, which is the normal way a Rust
//! package has both a lib and a bin target.
//!
//! `vakt-update` (the OS image updater) fetches and verifies an update
//! bundle the same way `zrpkg` fetches and verifies a package - same
//! `download_package`, same `verify_signature`, same `PackageManifest`, same
//! `safe_relative` archive-path check, same repository URL resolution via
//! `config::load`. An update bundle is packed and signed with the ordinary
//! `zrpkg pack` command; reusing this code instead of a second copy of it
//! means the fuzzing and tests already covering these functions cover the
//! update path too, rather than a second, unfuzzed implementation of the
//! same untrusted-input handling.

pub mod config;
pub mod db;
pub mod fetch;
pub mod manifest;
pub mod remove;
