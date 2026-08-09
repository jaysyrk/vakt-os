package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func writeTemp(t *testing.T, name, contents string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(path, []byte(contents), 0600); err != nil {
		t.Fatalf("writing fixture: %v", err)
	}
	return path
}

func TestNetStatusReportRendersConnectedLink(t *testing.T) {
	path := writeTemp(t, "vakt-net.status", strings.Join([]string{
		"state=connected",
		"interface=wlan0",
		"ssid=HomeNet",
		"ip=192.168.1.42",
		"detail=Link is up.",
	}, "\n"))

	got := netStatusReportFrom(path)
	for _, want := range []string{"connected", "wlan0", "HomeNet", "192.168.1.42"} {
		if !strings.Contains(got, want) {
			t.Errorf("report missing %q:\n%s", want, got)
		}
	}
}

func TestNetStatusReportHandlesMissingFile(t *testing.T) {
	got := netStatusReportFrom(filepath.Join(t.TempDir(), "absent"))
	if !strings.Contains(got, "has not reported yet") {
		t.Errorf("expected a not-reported message, got:\n%s", got)
	}
}

func TestServicesReportParsesSupervisorOutput(t *testing.T) {
	// Tab-separated, matching Supervisor::write_status in vakt-init:
	// name, state, pid, restarts, readiness, detail.
	path := writeTemp(t, "services.status",
		"vakt-net\trunning\t412\t0\tready\t10.0.2.15 on eth0\n"+
			"vakt-ids\tfailed\t\t6\twaiting\tFilesystem integrity monitor\n")

	got := servicesReportFrom(path)
	for _, want := range []string{
		"vakt-net", "running", "412", "ready", "10.0.2.15 on eth0",
		"vakt-ids", "failed", "waiting",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("report missing %q:\n%s", want, got)
		}
	}
	// A service with no PID should render a placeholder, not an empty column.
	if !strings.Contains(got, "-") {
		t.Errorf("expected a placeholder for the missing pid:\n%s", got)
	}
}

// A status file written by an older supervisor has five fields, not six. It
// should still render rather than being dropped or panicking on the index.
func TestServicesReportToleratesAShorterOlderLine(t *testing.T) {
	path := writeTemp(t, "services.status",
		"vakt-net\trunning\t412\t0\tWi-Fi and DHCP negotiation\n")

	got := servicesReportFrom(path)
	if !strings.Contains(got, "vakt-net") || !strings.Contains(got, "running") {
		t.Errorf("short line was not rendered:\n%s", got)
	}
}

func TestIDSReportShowsOnlyTheMostRecentAlerts(t *testing.T) {
	var lines []string
	for _, name := range []string{"one", "two", "three", "four"} {
		lines = append(lines, "2026-08-05T07:30:17Z\tMODIFIED\t/persistent/"+name)
	}
	path := writeTemp(t, "vakt-ids.alerts", strings.Join(lines, "\n")+"\n")

	got := idsReportFrom(path, 2)
	if strings.Contains(got, "/persistent/one") {
		t.Errorf("oldest alert should have been trimmed:\n%s", got)
	}
	if !strings.Contains(got, "/persistent/four") {
		t.Errorf("newest alert should be present:\n%s", got)
	}
}

func TestIDSReportHandlesMissingFile(t *testing.T) {
	got := idsReportFrom(filepath.Join(t.TempDir(), "absent"), 10)
	if !strings.Contains(got, "No alerts recorded") {
		t.Errorf("expected the no-alerts message, got:\n%s", got)
	}
}

func TestReadKeyValueFileIgnoresJunkLines(t *testing.T) {
	path := writeTemp(t, "kv", "state=connected\nnot a pair\n\nip=10.0.2.15\n")
	got := readKeyValueFile(path)

	if got["state"] != "connected" || got["ip"] != "10.0.2.15" {
		t.Errorf("unexpected parse result: %#v", got)
	}
	if len(got) != 2 {
		t.Errorf("expected exactly 2 keys, got %#v", got)
	}
}

// An alert file the panel cannot read must never render as an all-clear.
//
// vakt-ids runs as root and used to create /run/vakt-ids.alerts as 0600, so
// the panel - which runs unprivileged - got EACCES and displayed a green "No
// alerts recorded." on an appliance that may have been recording findings the
// whole time. Failing safe here means saying "I cannot tell", not "all fine".
func TestAnUnreadableAlertFileIsNotReportedAsNoAlerts(t *testing.T) {
	check := func(what, path string) {
		t.Helper()
		got := idsReportFrom(path, 10)
		if strings.Contains(got, "No alerts recorded") {
			t.Errorf("%s: rendered as an all-clear:\n%s", what, got)
		}
		if !strings.Contains(got, "Cannot read") {
			t.Errorf("%s: should say it cannot read the file:\n%s", what, got)
		}
		if !strings.Contains(got, "not an all-clear") {
			t.Errorf("%s: should say plainly that this is not an all-clear:\n%s", what, got)
		}
	}

	// EISDIR rather than EACCES, so the branch is still exercised when the
	// suite runs as root - which is how CI runs it.
	dir := filepath.Join(t.TempDir(), "vakt-ids.alerts")
	if err := os.Mkdir(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	check("a path that is not a regular file", dir)

	if os.Geteuid() != 0 {
		denied := filepath.Join(t.TempDir(), "vakt-ids.alerts")
		if err := os.WriteFile(denied, []byte("2026-01-01\tMODIFIED\t/persistent/etc/passwd\n"), 0o600); err != nil {
			t.Fatal(err)
		}
		if err := os.Chmod(denied, 0o000); err != nil {
			t.Fatal(err)
		}
		check("an unreadable file", denied)
	}
}

// vakt-init pre-creates the alert file empty so the panel can read it. Empty
// genuinely means nothing has been reported, and must read as such rather
// than as one blank alert.
func TestAnEmptyAlertFileReadsAsNoAlerts(t *testing.T) {
	path := filepath.Join(t.TempDir(), "vakt-ids.alerts")
	if err := os.WriteFile(path, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	got := idsReportFrom(path, 10)
	if !strings.Contains(got, "No alerts recorded") {
		t.Errorf("an empty alert file should read as no alerts, got:\n%s", got)
	}
}
