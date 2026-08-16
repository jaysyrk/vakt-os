package main

import (
	"os"
	"path/filepath"
	"testing"
)

func writePasswd(t *testing.T, contents string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "passwd")
	if err := os.WriteFile(path, []byte(contents), 0644); err != nil {
		t.Fatal(err)
	}
	return path
}

func TestCheckRootUIDsPassesWhenOnlyRootHasUIDZero(t *testing.T) {
	path := writePasswd(t, "root:x:0:0:root:/root:/bin/sh\nvakt:x:1000:1000::/home/vakt:/bin/sh\n")
	result := checkRootUIDsAt(path)
	if !result.Passed {
		t.Errorf("expected pass, got: %s", result.Description)
	}
}

func TestCheckRootUIDsFailsOnASecondUIDZeroAccount(t *testing.T) {
	path := writePasswd(t, "root:x:0:0:root:/root:/bin/sh\nbackdoor:x:0:0::/root:/bin/sh\n")
	result := checkRootUIDsAt(path)
	if result.Passed {
		t.Error("a second account sharing UID 0 must fail the check")
	}
}

func TestCheckRootUIDsFailsWhenUIDZeroIsNotNamedRoot(t *testing.T) {
	path := writePasswd(t, "toor:x:0:0::/root:/bin/sh\nvakt:x:1000:1000::/home/vakt:/bin/sh\n")
	result := checkRootUIDsAt(path)
	if result.Passed {
		t.Error("UID 0 under a name other than root must fail the check")
	}
}

func TestCheckRootUIDsFailsWithNoUIDZeroAtAll(t *testing.T) {
	path := writePasswd(t, "vakt:x:1000:1000::/home/vakt:/bin/sh\n")
	result := checkRootUIDsAt(path)
	if result.Passed {
		t.Error("a passwd file with no root account must fail the check")
	}
}

func TestCheckRootUIDsFailsOnAMissingFile(t *testing.T) {
	result := checkRootUIDsAt(filepath.Join(t.TempDir(), "absent"))
	if result.Passed {
		t.Error("a missing passwd file must fail, not silently pass")
	}
}

func TestCheckShadowPermissionsPassesWhenRestricted(t *testing.T) {
	// The check wants uid 0, and only a root run can produce a fixture it
	// owns. Skipping beats failing every contributor's test run.
	if os.Geteuid() != 0 {
		t.Skip("needs root: the fixture has to be owned by uid 0")
	}
	path := filepath.Join(t.TempDir(), "shadow")
	if err := os.WriteFile(path, []byte("root:*:19000:0:99999:7:::\n"), 0600); err != nil {
		t.Fatal(err)
	}
	result := checkShadowPermissionsAt(path)
	if !result.Passed {
		t.Errorf("expected pass for a root-owned, mode-0600 file, got: %s", result.Description)
	}
}

func TestCheckShadowPermissionsFailsWhenWorldReadable(t *testing.T) {
	path := filepath.Join(t.TempDir(), "shadow")
	if err := os.WriteFile(path, []byte("root:*:19000:0:99999:7:::\n"), 0644); err != nil {
		t.Fatal(err)
	}
	result := checkShadowPermissionsAt(path)
	if result.Passed {
		t.Error("a world-readable shadow file must fail the check")
	}
}

func TestCheckShadowPermissionsFailsOnAMissingFile(t *testing.T) {
	result := checkShadowPermissionsAt(filepath.Join(t.TempDir(), "absent"))
	if result.Passed {
		t.Error("a missing shadow file must fail, not silently pass")
	}
}

func writeSysctl(t *testing.T, root, path, value string) {
	t.Helper()
	full := filepath.Join(root, path)
	if err := os.MkdirAll(filepath.Dir(full), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(full, []byte(value), 0644); err != nil {
		t.Fatal(err)
	}
}

func TestCheckSysctlHardeningPassesWhenEveryPresentValueIsHardened(t *testing.T) {
	root := t.TempDir()
	checks := []hardeningSysctl{
		{"/proc/sys/kernel/dmesg_restrict", "1"},
		{"/proc/sys/kernel/kptr_restrict", "2"},
	}
	writeSysctl(t, root, checks[0].path, "1")
	writeSysctl(t, root, checks[1].path, "2")

	result := checkSysctlHardeningIn(root, checks)
	if !result.Passed {
		t.Errorf("expected pass, got: %s", result.Description)
	}
}

func TestCheckSysctlHardeningFailsWhenAPresentValueIsWrong(t *testing.T) {
	root := t.TempDir()
	checks := []hardeningSysctl{{"/proc/sys/kernel/dmesg_restrict", "1"}}
	writeSysctl(t, root, checks[0].path, "0")

	result := checkSysctlHardeningIn(root, checks)
	if result.Passed {
		t.Error("a sysctl left at its unhardened value must fail the check")
	}
}

// A kernel built without a given sysctl (an optional feature, a stripped-down
// build) is not a misconfiguration - the check should skip it, not fail.
func TestCheckSysctlHardeningSkipsSysctlsTheKernelDoesNotCarry(t *testing.T) {
	root := t.TempDir()
	checks := []hardeningSysctl{
		{"/proc/sys/kernel/dmesg_restrict", "1"},
		{"/proc/sys/does/not/exist", "1"},
	}
	writeSysctl(t, root, checks[0].path, "1")

	result := checkSysctlHardeningIn(root, checks)
	if !result.Passed {
		t.Errorf("a missing (not wrong) sysctl must not fail the check, got: %s", result.Description)
	}
}

func TestCheckSysctlHardeningFailsWhenNoneArePresent(t *testing.T) {
	root := t.TempDir()
	checks := []hardeningSysctl{{"/proc/sys/does/not/exist", "1"}}

	result := checkSysctlHardeningIn(root, checks)
	if result.Passed {
		t.Error("a kernel with none of the hardening sysctls should not report a pass")
	}
}

func TestHardeningSysctlsListHasNoDuplicatePaths(t *testing.T) {
	seen := map[string]bool{}
	for _, c := range hardeningSysctls {
		if seen[c.path] {
			t.Errorf("duplicate sysctl path in the real list: %s", c.path)
		}
		seen[c.path] = true
	}
}
