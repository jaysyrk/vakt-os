const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const root_module = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
        // Debug info is most of an optimized build's size here (4.2MB vs
        // 701KB) and nothing in the shipped image reads it. The panic message
        // a safety check produces is a runtime string and still prints; only
        // the stack trace loses its symbols.
        .strip = optimize != .Debug,
    });

    const exe = b.addExecutable(.{
        .name = "vakt-verify",
        .root_module = root_module,
    });
    b.installArtifact(exe);

    const unit_tests = b.addTest(.{ .root_module = root_module });
    const run_unit_tests = b.addRunArtifact(unit_tests);
    const test_step = b.step("test", "Run unit tests");
    test_step.dependOn(&run_unit_tests.step);
}
