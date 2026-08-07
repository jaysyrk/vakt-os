#![no_main]

use libfuzzer_sys::fuzz_target;
use zrpkg::manifest::PackageManifest;

// A manifest comes straight off the network, so nothing here should panic on
// arbitrary bytes - malformed JSON must always come back as an Err.
fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = PackageManifest::parse(s);
});
