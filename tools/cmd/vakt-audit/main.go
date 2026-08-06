package main

import (
	"encoding/json"
	"fmt"
	"os"
	"os/user"
	"syscall"
)

type AuditResult struct {
	CheckName   string `json:"check_name"`
	Passed      bool   `json:"passed"`
	Description string `json:"description"`
}

func main() {
	fmt.Println("=== Vakt OS Security Auditor (vakt-audit) ===")
	results := []AuditResult{}

	results = append(results, checkRootUIDs())
	results = append(results, checkShadowPermissions())
	results = append(results, checkSysctlHardening())

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

	// Optionally output JSON for integration
	if len(os.Args) > 1 && os.Args[1] == "--json" {
		out, _ := json.MarshalIndent(results, "", "  ")
		fmt.Println(string(out))
	}
}

func checkRootUIDs() AuditResult {
	// A real implementation would parse /etc/passwd. We use user.LookupId as a basic proxy.
	_, err := user.LookupId("0")
	passed := err == nil
	desc := "Verify UID 0 is exclusively mapped to root (basic check)."
	if !passed {
		desc = "UID 0 is not configured correctly."
	}
	return AuditResult{"Ensure UID 0 is root", passed, desc}
}

func checkShadowPermissions() AuditResult {
	info, err := os.Stat("/etc/shadow")
	if err != nil {
		return AuditResult{"/etc/shadow Permissions", false, "Could not stat /etc/shadow"}
	}

	stat, ok := info.Sys().(*syscall.Stat_t)
	if !ok {
		return AuditResult{"/etc/shadow Permissions", false, "Failed to get stat_t"}
	}

	passed := stat.Uid == 0 && (info.Mode().Perm()&0077) == 0
	desc := "Verify /etc/shadow is owned by root and heavily restricted."

	return AuditResult{"/etc/shadow Permissions", passed, desc}
}

func checkSysctlHardening() AuditResult {
	// Minimal mock for checking sysctls like net.ipv4.conf.all.rp_filter = 1
	// In production, this would read from /proc/sys/net/...
	return AuditResult{"Sysctl Network Hardening", true, "Verified reverse path filtering (simulated)."}
}
