#![no_main]

use libfuzzer_sys::fuzz_target;
use std::path::{Component, Path};
use zrpkg::remove::safe_relative;

// The property under test: whatever safe_relative accepts must actually be
// safe to join onto an install root - no component that could resolve
// outside it. A fuzz-found input that violates this would be a real path
// traversal bug in the code that decides what an archive is allowed to
// write, not just a panic.
fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let path = Path::new(s);

    if let Ok(relative) = safe_relative(path) {
        for component in relative.components() {
            assert!(
                !matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                ),
                "safe_relative accepted a path that resolves outside the root: {:?} -> {:?}",
                path,
                relative
            );
        }
    }
});
