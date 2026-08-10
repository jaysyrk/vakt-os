package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func alertLine(when time.Time, kind, detail string) string {
	return fmt.Sprintf("%s\t%s\t%s", when.Format(time.RFC3339), kind, detail)
}

func TestParseAlertLine(t *testing.T) {
	when := time.Date(2026, 8, 9, 23, 27, 14, 0, time.UTC)
	got, ok := parseAlertLine(alertLine(when, "ADDED", "/persistent/etc/ids-test.txt"))
	if !ok {
		t.Fatal("a well-formed line should parse")
	}
	if !got.HasTime || !got.When.Equal(when) {
		t.Errorf("timestamp: got %v (hasTime=%v)", got.When, got.HasTime)
	}
	if got.Kind != "ADDED" || got.Detail != "/persistent/etc/ids-test.txt" {
		t.Errorf("got kind=%q detail=%q", got.Kind, got.Detail)
	}

	if _, ok := parseAlertLine("   "); ok {
		t.Error("a blank line is not a finding")
	}
}

// The alert file is the durable record; nothing in it may be silently dropped.
func TestAnUnparseableLineIsKeptNotDropped(t *testing.T) {
	got, ok := parseAlertLine("something totally unexpected")
	if !ok {
		t.Fatal("an unparseable line must still be reported")
	}
	if !strings.Contains(got.Detail, "something totally unexpected") {
		t.Errorf("the original text should survive, got %q", got.Detail)
	}

	// A tab-separated line whose timestamp is junk keeps its kind and detail.
	got, ok = parseAlertLine("not-a-time\tMODIFIED\t/persistent/etc/passwd")
	if !ok {
		t.Fatal("should still be reported")
	}
	if got.HasTime {
		t.Error("an unparseable timestamp must not be presented as a real one")
	}
	if got.Kind != "MODIFIED" || got.Detail != "/persistent/etc/passwd" {
		t.Errorf("got kind=%q detail=%q", got.Kind, got.Detail)
	}
}

func TestCountByKind(t *testing.T) {
	now := time.Now()
	alerts := parseAlerts(strings.Join([]string{
		alertLine(now, "ADDED", "/a"),
		alertLine(now, "ADDED", "/b"),
		alertLine(now, "MODIFIED", "/c"),
	}, "\n"))

	counts := countByKind(alerts)
	if counts["ADDED"] != 2 || counts["MODIFIED"] != 1 || counts["DELETED"] != 0 {
		t.Errorf("got %v", counts)
	}
}

func TestActivityBuckets(t *testing.T) {
	now := time.Date(2026, 8, 9, 12, 0, 0, 0, time.UTC)
	alerts := parseAlerts(strings.Join([]string{
		alertLine(now.Add(-90*time.Second), "ADDED", "/a"), // 2 buckets back
		alertLine(now.Add(-30*time.Second), "ADDED", "/b"), // newest bucket
		alertLine(now.Add(-20*time.Second), "ADDED", "/c"), // newest bucket
		alertLine(now.Add(-48*time.Hour), "ADDED", "/old"), // outside the window
	}, "\n"))

	got := activityBuckets(alerts, now, time.Minute, 5)
	if len(got) != 5 {
		t.Fatalf("expected 5 buckets, got %d", len(got))
	}
	if got[4] != 2 {
		t.Errorf("the two recent findings belong in the newest bucket, got %v", got)
	}
	if got[3] != 1 {
		t.Errorf("the 90s-old finding belongs one bucket back, got %v", got)
	}

	total := 0
	for _, c := range got {
		total += c
	}
	if total != 3 {
		t.Errorf("a finding from two days ago must not appear in the last five minutes: %v", got)
	}
}

func TestUndatedFindingsAreNotPlottedAsRecent(t *testing.T) {
	now := time.Now()
	alerts := parseAlerts("not-a-time\tMODIFIED\t/persistent/etc/passwd")
	buckets := activityBuckets(alerts, now, time.Minute, 10)
	for i, c := range buckets {
		if c != 0 {
			t.Errorf("bucket %d should be empty, got %d", i, c)
		}
	}
}

func TestSparkline(t *testing.T) {
	if got := sparkline([]int{0, 0, 0}); got != "▁▁▁" {
		t.Errorf("a quiet window should be flat, got %q", got)
	}

	got := sparkline([]int{0, 1, 9})
	if []rune(got)[0] != '▁' {
		t.Errorf("an empty bucket should be the lowest block, got %q", got)
	}
	if []rune(got)[1] == '▁' {
		t.Errorf("a bucket with a finding must be visibly above empty, got %q", got)
	}
	if []rune(got)[2] != '█' {
		t.Errorf("the busiest bucket should be full height, got %q", got)
	}
	if len([]rune(sparkline([]int{1, 2, 3, 4}))) != 4 {
		t.Error("one column per bucket")
	}
}

// A file it cannot read must not render as a calm empty dashboard.
func TestMonitorDoesNotShowAnAllClearWhenItCannotRead(t *testing.T) {
	dir := t.TempDir()
	unreadable := filepath.Join(dir, "alerts")
	if err := os.Mkdir(unreadable, 0o755); err != nil { // EISDIR, works as root too
		t.Fatal(err)
	}

	report := idsMonitorReport(unreadable, filepath.Join(dir, "no-status"), time.Now())
	if strings.Contains(report, "No findings recorded") {
		t.Errorf("rendered as an all-clear:\n%s", report)
	}
	if !strings.Contains(report, "Cannot read") || !strings.Contains(report, "not an all-clear") {
		t.Errorf("should say plainly that it cannot see the file:\n%s", report)
	}
}

func TestMonitorReportsFindings(t *testing.T) {
	dir := t.TempDir()
	alerts := filepath.Join(dir, "alerts")
	now := time.Now()
	body := strings.Join([]string{
		alertLine(now.Add(-2*time.Minute), "ADDED", "/persistent/etc/ids-test.txt"),
		alertLine(now.Add(-1*time.Minute), "MODIFIED", "/persistent/etc/passwd"),
	}, "\n") + "\n"
	if err := os.WriteFile(alerts, []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}

	status := filepath.Join(dir, "services.status")
	if err := os.WriteFile(status,
		[]byte("vakt-ids\trunning\t199\t0\tready\twatching 2 file(s) under /persistent\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	report := idsMonitorReport(alerts, status, now)
	for _, want := range []string{
		"/persistent/etc/passwd", // the finding itself
		"MODIFIED",               // its kind
		"watching 2 file(s)",     // the daemon's own reported status
		"ready",                  // that it is up
		"2 finding(s) on record", // the total
	} {
		if !strings.Contains(report, want) {
			t.Errorf("expected %q in:\n%s", want, report)
		}
	}

	// Newest first: the most recent finding should appear before the older one.
	if strings.Index(report, "passwd") > strings.Index(report, "ids-test.txt") {
		t.Errorf("findings should be newest first:\n%s", report)
	}
}

// vakt-init creates this file empty at boot; empty means quiet, not broken.
func TestMonitorTreatsAnEmptyFileAsQuiet(t *testing.T) {
	dir := t.TempDir()
	alerts := filepath.Join(dir, "alerts")
	if err := os.WriteFile(alerts, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	report := idsMonitorReport(alerts, filepath.Join(dir, "none"), time.Now())
	if !strings.Contains(report, "No findings recorded") {
		t.Errorf("an empty alert file is quiet, not broken:\n%s", report)
	}
}

func TestMonitorSaysWhenTheDaemonIsNotRunning(t *testing.T) {
	dir := t.TempDir()
	status := filepath.Join(dir, "services.status")
	if err := os.WriteFile(status,
		[]byte("vakt-net\trunning\t197\t0\tready\tno network configured\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	got := idsDaemonLine(status)
	if !strings.Contains(got, "not supervised") {
		t.Errorf("expected a warning that vakt-ids is absent, got %q", got)
	}
}
