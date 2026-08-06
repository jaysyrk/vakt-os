//! vakt-verify: an independent check on a package's signature.
//!
//! zrpkg (Rust, ed25519-dalek) is the installer and the thing that actually
//! decides whether a package goes onto the system. This tool answers the same
//! question - does this archive's SHA-256 digest carry a valid Ed25519
//! signature under this public key? - using a completely separate
//! implementation: a different language, a different crypto library, only the
//! Zig standard library, no shared code with zrpkg at all.
//!
//! That independence is the point. A bug or a backdoor in one verifier is not
//! automatically a bug in the other, so agreement between the two is stronger
//! evidence than either check alone - the same reason a security-sensitive
//! release gets signed off by more than one person. This tool changes nothing
//! on the system and holds no trust anchor of its own; it reads the same
//! `/etc/vakt/trusted.key` zrpkg does and reports pass or fail.
//!
//! The message that gets verified is deliberately not the raw archive bytes:
//! it is SHA-256(archive), matching exactly what `pkg-manager/src/fetch.rs`
//! signs. Verifying different bytes than the installer does would make this
//! tool agree or disagree with zrpkg for the wrong reasons.

const std = @import("std");
const Ed25519 = std.crypto.sign.Ed25519;
const Sha256 = std.crypto.hash.sha2.Sha256;

/// Where the trust anchor lives on a built system. Overridable with
/// --pubkey/--pubkey-file for testing against a repository's own key before
/// it is ever installed into an image.
const DEFAULT_TRUSTED_KEY = "/etc/vakt/trusted.key";

/// Archives larger than this are refused before they are read into memory.
/// Nothing this project ships is anywhere near the limit; it exists so a
/// hostile or corrupt file cannot be used to exhaust memory.
const MAX_ARCHIVE_BYTES: usize = 512 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 1 * 1024 * 1024;
const MAX_KEY_FILE_BYTES: usize = 4096;

const Manifest = struct {
    name: []const u8 = "",
    version: []const u8 = "",
    signature: []const u8,
};

const Args = struct {
    archive_path: []const u8,
    manifest_path: []const u8,
    pubkey_hex: ?[]const u8 = null,
    pubkey_path: ?[]const u8 = null,
};

const UsageError = error{BadUsage};

fn parseArgs(argv: []const [:0]const u8) UsageError!Args {
    var archive_path: ?[]const u8 = null;
    var manifest_path: ?[]const u8 = null;
    var pubkey_hex: ?[]const u8 = null;
    var pubkey_path: ?[]const u8 = null;

    var i: usize = 1;
    while (i < argv.len) : (i += 1) {
        const arg = argv[i];
        if (std.mem.eql(u8, arg, "--pubkey")) {
            i += 1;
            if (i >= argv.len) return error.BadUsage;
            pubkey_hex = argv[i];
        } else if (std.mem.eql(u8, arg, "--pubkey-file")) {
            i += 1;
            if (i >= argv.len) return error.BadUsage;
            pubkey_path = argv[i];
        } else if (archive_path == null) {
            archive_path = arg;
        } else if (manifest_path == null) {
            manifest_path = arg;
        } else {
            return error.BadUsage;
        }
    }

    return Args{
        .archive_path = archive_path orelse return error.BadUsage,
        .manifest_path = manifest_path orelse return error.BadUsage,
        .pubkey_hex = pubkey_hex,
        .pubkey_path = pubkey_path,
    };
}

fn printUsage(program_name: []const u8) void {
    std.debug.print(
        \\usage: {s} <archive.zrp> <manifest.json> [--pubkey <hex> | --pubkey-file <path>]
        \\
        \\Independently re-verifies a Vakt OS package signature: recomputes the
        \\archive's SHA-256 digest and checks it against the manifest's Ed25519
        \\signature, using only the Zig standard library - no code shared with
        \\zrpkg's own (Rust) verifier.
        \\
        \\With neither --pubkey option, the key is read from $ZRPKG_PUBKEY or
        \\from {s}, matching zrpkg's own precedence.
        \\
        \\Exit status: 0 if the signature is valid, 1 otherwise.
        \\
    , .{ program_name, DEFAULT_TRUSTED_KEY });
}

/// Decodes a hex string, tolerating and trimming surrounding whitespace since
/// that is exactly what a hand-edited key file tends to contain.
///
/// Deliberately silent on failure - a helper has no useful context to add to
/// the error, and every call site already knows what it was trying to decode
/// and can say so.
fn decodeHex(allocator: std.mem.Allocator, text: []const u8, expected_len: usize) ![]u8 {
    const trimmed = std.mem.trim(u8, text, &std.ascii.whitespace);
    if (trimmed.len != expected_len * 2) return error.WrongLength;

    const out = try allocator.alloc(u8, expected_len);
    errdefer allocator.free(out);
    _ = try std.fmt.hexToBytes(out, trimmed);
    return out;
}

/// Resolves the trusted public key: an explicit flag first, then the
/// environment, then the on-disk trust anchor - the same order zrpkg itself
/// checks in `pkg-manager/src/install.rs`.
fn resolvePublicKey(
    allocator: std.mem.Allocator,
    io: std.Io,
    environ_map: *const std.process.Environ.Map,
    args: Args,
) ![]u8 {
    if (args.pubkey_hex) |hex| {
        return decodeHex(allocator, hex, Ed25519.PublicKey.encoded_length) catch |err| {
            std.log.err("--pubkey is not a valid Ed25519 public key: {t}", .{err});
            return err;
        };
    }

    if (args.pubkey_path) |path| {
        const text = try std.Io.Dir.cwd().readFileAlloc(io, path, allocator, .limited(MAX_KEY_FILE_BYTES));
        defer allocator.free(text);
        return decodeHex(allocator, text, Ed25519.PublicKey.encoded_length) catch |err| {
            std.log.err("{s} does not contain a valid Ed25519 public key: {t}", .{ path, err });
            return err;
        };
    }

    if (environ_map.get("ZRPKG_PUBKEY")) |env_key| {
        if (env_key.len != 0) {
            return decodeHex(allocator, env_key, Ed25519.PublicKey.encoded_length) catch |err| {
                std.log.err("$ZRPKG_PUBKEY is not a valid Ed25519 public key: {t}", .{err});
                return err;
            };
        }
    }

    const text = std.Io.Dir.cwd().readFileAlloc(io, DEFAULT_TRUSTED_KEY, allocator, .limited(MAX_KEY_FILE_BYTES)) catch |err| {
        std.log.err("no trusted key: could not read {s} ({t})", .{ DEFAULT_TRUSTED_KEY, err });
        return error.NoTrustedKey;
    };
    defer allocator.free(text);
    return decodeHex(allocator, text, Ed25519.PublicKey.encoded_length) catch |err| {
        std.log.err("{s} does not contain a valid Ed25519 public key: {t}", .{ DEFAULT_TRUSTED_KEY, err });
        return err;
    };
}

/// Taking `std.process.Init` as the sole parameter is how the 0.16 runtime
/// recognises a program that wants its allocator, `Io` implementation and
/// arguments handed to it already assembled, instead of building them by
/// hand - this is the process-wide `Io.Threaded` the runtime itself sets up,
/// not a second one.
pub fn main(init: std.process.Init) u8 {
    const argv = init.minimal.args.toSlice(init.arena.allocator()) catch |err| {
        std.log.err("could not read arguments: {t}", .{err});
        return 1;
    };

    const args = parseArgs(argv) catch {
        printUsage(if (argv.len > 0) argv[0] else "vakt-verify");
        return 2;
    };

    return run(init.gpa, init.io, init.environ_map, args) catch |err| {
        std.log.err("{t}", .{err});
        return 1;
    };
}

fn run(allocator: std.mem.Allocator, io: std.Io, environ_map: *const std.process.Environ.Map, args: Args) !u8 {
    const cwd = std.Io.Dir.cwd();

    const archive = cwd.readFileAlloc(io, args.archive_path, allocator, .limited(MAX_ARCHIVE_BYTES)) catch |err| {
        std.log.err("reading {s}: {t}", .{ args.archive_path, err });
        return 1;
    };
    defer allocator.free(archive);

    const manifest_text = cwd.readFileAlloc(io, args.manifest_path, allocator, .limited(MAX_MANIFEST_BYTES)) catch |err| {
        std.log.err("reading {s}: {t}", .{ args.manifest_path, err });
        return 1;
    };
    defer allocator.free(manifest_text);

    var parsed = std.json.parseFromSlice(
        Manifest,
        allocator,
        manifest_text,
        .{ .ignore_unknown_fields = true },
    ) catch |err| {
        std.log.err("parsing {s}: {t}", .{ args.manifest_path, err });
        return 1;
    };
    defer parsed.deinit();
    const manifest = parsed.value;

    const signature_bytes = decodeHex(allocator, manifest.signature, Ed25519.Signature.encoded_length) catch {
        std.log.err("manifest signature is not {d} bytes of hex", .{Ed25519.Signature.encoded_length});
        return 1;
    };
    defer allocator.free(signature_bytes);

    const pubkey_bytes = resolvePublicKey(allocator, io, environ_map, args) catch {
        return 1;
    };
    defer allocator.free(pubkey_bytes);

    // The exact quantity zrpkg signs: not the archive itself, but its
    // SHA-256 digest. See pkg-manager/src/fetch.rs::verify_signature.
    var digest: [Sha256.digest_length]u8 = undefined;
    Sha256.hash(archive, &digest, .{});

    const public_key = Ed25519.PublicKey.fromBytes(pubkey_bytes[0..32].*) catch |err| {
        std.log.err("public key is not a valid Ed25519 point: {t}", .{err});
        return 1;
    };
    const signature = Ed25519.Signature.fromBytes(signature_bytes[0..64].*);

    signature.verify(&digest, public_key) catch |err| {
        std.debug.print(
            "FAIL  {s}  sha256:{s}  {t}\n",
            .{ displayName(&manifest, args.archive_path), std.fmt.bytesToHex(digest, .lower), err },
        );
        return 1;
    };

    std.debug.print(
        "PASS  {s}  sha256:{s}\n",
        .{ displayName(&manifest, args.archive_path), std.fmt.bytesToHex(digest, .lower) },
    );
    return 0;
}

fn displayName(manifest: *const Manifest, fallback: []const u8) []const u8 {
    if (manifest.name.len == 0) return fallback;
    return manifest.name;
}

test "parseArgs accepts the positional archive and manifest" {
    const argv = [_][:0]const u8{ "vakt-verify", "a.zrp", "a.json" };
    const args = try parseArgs(&argv);
    try std.testing.expectEqualStrings("a.zrp", args.archive_path);
    try std.testing.expectEqualStrings("a.json", args.manifest_path);
    try std.testing.expect(args.pubkey_hex == null);
    try std.testing.expect(args.pubkey_path == null);
}

test "parseArgs reads --pubkey and --pubkey-file" {
    const with_hex = [_][:0]const u8{ "vakt-verify", "a.zrp", "a.json", "--pubkey", "ab12" };
    const hex_args = try parseArgs(&with_hex);
    try std.testing.expectEqualStrings("ab12", hex_args.pubkey_hex.?);

    const with_file = [_][:0]const u8{ "vakt-verify", "a.zrp", "a.json", "--pubkey-file", "/etc/vakt/trusted.key" };
    const file_args = try parseArgs(&with_file);
    try std.testing.expectEqualStrings("/etc/vakt/trusted.key", file_args.pubkey_path.?);
}

test "parseArgs rejects missing operands and dangling flags" {
    try std.testing.expectError(error.BadUsage, parseArgs(&[_][:0]const u8{"vakt-verify"}));
    try std.testing.expectError(error.BadUsage, parseArgs(&[_][:0]const u8{ "vakt-verify", "a.zrp" }));
    try std.testing.expectError(
        error.BadUsage,
        parseArgs(&[_][:0]const u8{ "vakt-verify", "a.zrp", "a.json", "--pubkey" }),
    );
    try std.testing.expectError(
        error.BadUsage,
        parseArgs(&[_][:0]const u8{ "vakt-verify", "a.zrp", "a.json", "extra" }),
    );
}

test "decodeHex round-trips and rejects the wrong length" {
    const allocator = std.testing.allocator;

    const decoded = try decodeHex(allocator, "  deadbeef  \n", 4);
    defer allocator.free(decoded);
    try std.testing.expectEqualSlices(u8, &[_]u8{ 0xde, 0xad, 0xbe, 0xef }, decoded);

    try std.testing.expectError(error.WrongLength, decodeHex(allocator, "ab", 4));
    try std.testing.expectError(error.WrongLength, decodeHex(allocator, "deadbeefff", 4));
}

test "displayName prefers the manifest name over the archive path" {
    const named = Manifest{ .name = "vakt-audit", .signature = "" };
    try std.testing.expectEqualStrings("vakt-audit", displayName(&named, "/tmp/whatever.zrp"));

    const unnamed = Manifest{ .signature = "" };
    try std.testing.expectEqualStrings("/tmp/whatever.zrp", displayName(&unnamed, "/tmp/whatever.zrp"));
}

test "a signature verifies only over exactly what was signed" {
    // Not a cross-check against zrpkg - that lives in the shell-level
    // integration test in CI, where a real .zrp is packed and signed by the
    // Rust implementation and checked here. This test is narrower: it pins
    // down that this binary verifies the *hash*, not the raw message, and
    // that flipping a single byte anywhere invalidates it. Both properties
    // are exactly what would make this tool agree with zrpkg for the wrong
    // reasons if they were ever violated silently.
    // A fixed seed, not real randomness: a unit test should be reproducible,
    // and the property under test does not depend on which keypair it is.
    const seed: [Ed25519.KeyPair.seed_length]u8 = [_]u8{0x42} ** Ed25519.KeyPair.seed_length;
    const key_pair = try Ed25519.KeyPair.generateDeterministic(seed);

    const archive_bytes = "a fake archive, just needs to be bytes";
    var digest: [Sha256.digest_length]u8 = undefined;
    Sha256.hash(archive_bytes, &digest, .{});

    const signature = try key_pair.sign(&digest, null);

    try signature.verify(&digest, key_pair.public_key);

    var other_archive_digest: [Sha256.digest_length]u8 = undefined;
    Sha256.hash("a different archive entirely", &other_archive_digest, .{});
    try std.testing.expectError(
        error.SignatureVerificationFailed,
        signature.verify(&other_archive_digest, key_pair.public_key),
    );

    try std.testing.expectError(
        error.SignatureVerificationFailed,
        signature.verify(archive_bytes, key_pair.public_key),
    );
}
