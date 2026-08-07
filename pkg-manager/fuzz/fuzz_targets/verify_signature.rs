#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use zrpkg::fetch::verify_signature;

#[derive(Debug, Arbitrary)]
struct Input {
    data: Vec<u8>,
    signature_hex: String,
    public_key_hex: String,
}

// All three fields are attacker-controlled: data is the downloaded archive,
// and both hex strings come from the manifest the same download supplies.
// Malformed hex, wrong lengths, and outright garbage all have to come back
// as an Err - never a panic.
fuzz_target!(|input: Input| {
    let _ = verify_signature(&input.data, &input.signature_hex, &input.public_key_hex);
});
