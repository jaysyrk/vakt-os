// vakt-audit is the Vakt OS security compliance auditor. It runs a small,
// fixed set of checks against the running system and reports a pass/fail
// score - a CIS-benchmark-style sanity check an operator or a fleet
// monitoring tool can run without knowing what any individual check does.
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"syscall"
)

type AuditResult struct {
	CheckName   string `json:"check_name"`
	Passed      bool   `json:"passed"`
	Description string `json:"description"`
}

func main() {
	fmt.Println("=== Vakt OS Security Auditor (vakt-audit) ===")
	results := []AuditResult{
		checkRootUIDs(),
		checkShadowPermissions(),
		checkSysctlHardening(),
	}

	passedCount := 0
	for _, r := range results {
		status := "[FAIL]"
		if r.Passed {
			status = "[PASS]"
			passedCount++
		}
		fmt.Printf("%s %s - %s\n", status, r.CheckName, r.Description)
	}

	fmt.Printf("\nCompliance Score: %d/%d Checks Passed\n", passedCount, len(results))

	if len(os.Args) > 1 && os.Args[1] == "--json" {
		out, _ := json.MarshalIndent(results, "", "  ")
		fmt.Println(string(out))
	}
}

func checkRootUIDs() AuditResult { return checkRootUIDsAt("/etc/passwd") }

// checkRootUIDsAt verifies UID 0 is mapped to exactly one account, and that
// the account is named root. That is a stronger claim than "an account with
// UID 0 exists": a second account sharing UID 0 - a common backdoor - would
// still pass that weaker check.
func checkRootUIDsAt(path string) AuditResult {
	name := "UID 0 Uniqueness"

	data, err := os.ReadFile(path)
	if err != nil {
		return AuditResult{name, false, fmt.Sprintf("Could not read %s: %v", path, err)}
	}

	var zeroUID []string
	for _, line := range strings.Split(string(data), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		fields := strings.Split(line, ":")
		if len(fields) < 3 {
			continue
		}
		if fields[2] == "0" {
			zeroUID = append(zeroUID, fields[0])
		}
	}

	switch {
	case len(zeroUID) == 0:
		return AuditResult{name, false, "No account has UID 0; the system has no root."}
	case len(zeroUID) == 1 && zeroUID[0] == "root":
		return AuditResult{name, true, "UID 0 is mapped to exactly one account, named root."}
	default:
		return AuditResult{name, false, fmt.Sprintf(
			"UID 0 is mapped to: %s (want exactly one account, named root)",
			strings.Join(zeroUID, ", "))}
	}
}

func checkShadowPermissions() AuditResult { return checkShadowPermissionsAt("/etc/shadow") }

func checkShadowPermissionsAt(path string) AuditResult {
	name := "/etc/shadow Permissions"

	info, err := os.Stat(path)
	if err != nil {
		return AuditResult{name, false, fmt.Sprintf("Could not stat %s: %v", path, err)}
	}

	stat, ok := info.Sys().(*syscall.Stat_t)
	if !ok {
		return AuditResult{name, false, "Could not read the file's owning uid on this platform."}
	}

	passed := stat.Uid == 0 && (info.Mode().Perm()&0077) == 0
	desc := "root-owned and unreadable by group or other, as it should be."
	if !passed {
		desc = fmt.Sprintf("owned by uid %d, mode %o (want uid 0, no group/other access)",
			stat.Uid, info.Mode().Perm())
	}
	return AuditResult{name, passed, desc}
}

type hardeningSysctl struct {
	path string
	want string
}

// hardeningSysctls mirrors the list vakt-init applies at boot, in
// vakt-init/src/sysctl.rs. Kept in sync by hand: this tool has to read the
// values independently of how they were set for the audit to mean anything.
var hardeningSysctls = []hardeningSysctl{
	{"/proc/sys/kernel/yama/ptrace_scope", "1"},
	{"/proc/sys/kernel/kptr_restrict", "2"},
	{"/proc/sys/kernel/dmesg_restrict", "1"},
	{"/proc/sys/net/ipv4/conf/all/rp_filter", "1"},
	{"/proc/sys/net/ipv4/conf/default/rp_filter", "1"},
	{"/proc/sys/net/ipv4/conf/all/accept_redirects", "0"},
	{"/proc/sys/net/ipv4/conf/default/accept_redirects", "0"},
	{"/proc/sys/net/ipv4/conf/all/accept_source_route", "0"},
	{"/proc/sys/net/ipv4/conf/default/accept_source_route", "0"},
	{"/proc/sys/net/ipv4/icmp_echo_ignore_broadcasts", "1"},
	{"/proc/sys/net/ipv4/tcp_syncookies", "1"},
}

func checkSysctlHardening() AuditResult {
	return checkSysctlHardeningIn("/", hardeningSysctls)
}

// checkSysctlHardeningIn reads each sysctl under root and compares it to the
// hardened value. A sysctl the running kernel does not carry at all is
// skipped rather than counted as a failure - a kernel built without a given
// knob is not a misconfiguration, and vakt-init's own hardening pass treats
// a missing path the same way.
func checkSysctlHardeningIn(root string, checks []hardeningSysctl) AuditResult {
	name := "Sysctl Hardening"

	var wrong []string
	checked := 0
	for _, c := range checks {
		data, err := os.ReadFile(filepath.Join(root, c.path))
		if err != nil {
			continue
		}
		checked++
		if strings.TrimSpace(string(data)) != c.want {
			wrong = append(wrong, c.path)
		}
	}

	if checked == 0 {
		return AuditResult{name, false, "None of the hardening sysctls are present on this kernel."}
	}
	if len(wrong) > 0 {
		return AuditResult{name, false, fmt.Sprintf(
			"%d/%d sysctls not at the hardened value: %s", len(wrong), checked, strings.Join(wrong, ", "))}
	}
	return AuditResult{name, true, fmt.Sprintf("%d/%d hardening sysctls verified.", checked, checked)}
}
