package main

import (
	"fmt"
	"os"
	"sort"
	"strings"
	"time"

	"github.com/rivo/tview"
)

const idsRefresh = time.Second

// Most interesting first; INFO is the daemon talking about itself.
var idsKinds = []string{"MODIFIED", "ADDED", "DELETED", "PERMISSIONS", "INFO"}

var idsKindColour = map[string]string{
	"MODIFIED":    "red",
	"ADDED":       "yellow",
	"DELETED":     "red",
	"PERMISSIONS": "orange",
	"INFO":        "green",
}

type idsAlert struct {
	When    time.Time
	HasTime bool
	Kind    string
	Detail  string
}

// parseAlertLine reads one tab-separated line: RFC3339 time, kind, detail.
// An unparseable line is kept with whatever text it has rather than dropped.
func parseAlertLine(line string) (idsAlert, bool) {
	line = strings.TrimRight(line, "\r")
	if strings.TrimSpace(line) == "" {
		return idsAlert{}, false
	}

	fields := strings.SplitN(line, "\t", 3)
	if len(fields) < 3 {
		return idsAlert{Kind: "?", Detail: strings.TrimSpace(line)}, true
	}

	alert := idsAlert{Kind: strings.TrimSpace(fields[1]), Detail: strings.TrimSpace(fields[2])}
	if when, err := time.Parse(time.RFC3339, strings.TrimSpace(fields[0])); err == nil {
		alert.When = when
		alert.HasTime = true
	}
	return alert, true
}

func parseAlerts(body string) []idsAlert {
	var alerts []idsAlert
	for _, line := range strings.Split(body, "\n") {
		if alert, ok := parseAlertLine(line); ok {
			alerts = append(alerts, alert)
		}
	}
	return alerts
}

func countByKind(alerts []idsAlert) map[string]int {
	counts := map[string]int{}
	for _, a := range alerts {
		counts[a.Kind]++
	}
	return counts
}

// activityBuckets counts findings per interval, oldest first. Undated ones are
// left out rather than placed at "now", which would invent activity.
func activityBuckets(alerts []idsAlert, now time.Time, width time.Duration, buckets int) []int {
	out := make([]int, buckets)
	if buckets == 0 || width <= 0 {
		return out
	}
	oldest := now.Add(-width * time.Duration(buckets))

	for _, a := range alerts {
		if !a.HasTime || a.When.Before(oldest) || a.When.After(now) {
			continue
		}
		idx := int(a.When.Sub(oldest) / width)
		if idx >= buckets {
			idx = buckets - 1
		}
		if idx < 0 {
			continue
		}
		out[idx]++
	}
	return out
}

// sparkline renders counts as blocks, scaled to the busiest bucket.
func sparkline(counts []int) string {
	blocks := []rune("▁▂▃▄▅▆▇█")
	peak := 0
	for _, c := range counts {
		if c > peak {
			peak = c
		}
	}

	var b strings.Builder
	for _, c := range counts {
		if c == 0 {
			b.WriteRune('▁')
			continue
		}
		// Rounded up so one finding never renders identically to none.
		top := len(blocks) - 1
		level := (c*top + peak - 1) / peak
		if level < 1 {
			level = 1
		}
		if level > top {
			level = top
		}
		b.WriteRune(blocks[level])
	}
	return b.String()
}

// idsMonitorReport renders the live view. Separate from the widget so it is
// testable without a terminal.
func idsMonitorReport(alertPath, statusPath string, now time.Time) string {
	var b strings.Builder

	fmt.Fprintf(&b, "[::b]vakt-ids[-]   %s\n\n", idsDaemonLine(statusPath))

	data, err := os.ReadFile(alertPath)
	if err != nil {
		if !os.IsNotExist(err) {
			// An unreadable file is not an all-clear; showing zeros here would
			// be worse than showing nothing.
			fmt.Fprintf(&b, "[red]Cannot read %s: %v[-]\n\n", alertPath, err)
			b.WriteString("[red]This is not an all-clear.[-] vakt-ids may be recording\n")
			b.WriteString("findings this page cannot see. Check it from a root shell.\n")
			return b.String()
		}
		b.WriteString("[green]No findings recorded.[-]\n\n")
		b.WriteString("vakt-ids writes to " + alertPath + " as it finds things.\n")
		return b.String()
	}

	alerts := parseAlerts(string(data))
	if len(alerts) == 0 {
		b.WriteString("[green]No findings recorded.[-]\n\n")
		b.WriteString("vakt-ids writes to " + alertPath + " as it finds things.\n")
		return b.String()
	}

	counts := countByKind(alerts)
	b.WriteString("  ")
	for _, kind := range idsKinds {
		colour := idsKindColour[kind]
		if counts[kind] == 0 {
			colour = "gray"
		}
		fmt.Fprintf(&b, "[%s]%s %d[-]   ", colour, kind, counts[kind])
	}
	// Kinds this panel predates are still shown.
	var unknown []string
	for kind := range counts {
		if _, known := idsKindColour[kind]; !known {
			unknown = append(unknown, kind)
		}
	}
	sort.Strings(unknown)
	for _, kind := range unknown {
		fmt.Fprintf(&b, "[white]%s %d[-]   ", kind, counts[kind])
	}
	b.WriteString("\n\n")

	fmt.Fprintf(&b, "  last hour  [aqua]%s[-]\n\n",
		sparkline(activityBuckets(alerts, now, time.Minute, 60)))

	b.WriteString("[::b]  Most recent[-]\n\n")
	for i := len(alerts) - 1; i >= 0 && i >= len(alerts)-20; i-- {
		a := alerts[i]
		colour, known := idsKindColour[a.Kind]
		if !known {
			colour = "white"
		}
		stamp := "        "
		if a.HasTime {
			stamp = a.When.Local().Format("15:04:05")
		}
		fmt.Fprintf(&b, "  [gray]%s[-]  [%s]%-11s[-]  %s\n", stamp, colour, a.Kind, a.Detail)
	}

	fmt.Fprintf(&b, "\n  [gray]%d finding(s) on record. Refreshing every %s.[-]\n",
		len(alerts), idsRefresh)
	return b.String()
}

// idsDaemonLine describes vakt-ids itself, from the supervisor's status file.
func idsDaemonLine(statusPath string) string {
	data, err := os.ReadFile(statusPath)
	if err != nil {
		return "[yellow]supervisor status unavailable[-]"
	}
	for _, line := range strings.Split(string(data), "\n") {
		fields := strings.Split(line, "\t")
		if len(fields) < 6 || fields[0] != "vakt-ids" {
			continue
		}
		state, readiness, detail := fields[1], fields[4], fields[5]
		colour := "red"
		if state == "running" && readiness == "ready" {
			colour = "green"
		} else if state == "running" {
			colour = "yellow"
		}
		return fmt.Sprintf("[%s]%s / %s[-]  [gray]%s[-]", colour, state, readiness, detail)
	}
	return "[red]not supervised - vakt-ids is not running[-]"
}

// idsMonitor builds the live view and the goroutine that redraws it. The
// ticker runs for the life of the panel: one small file read a second, and no
// stale first frame when the page is opened.
func idsMonitor(app *tview.Application, alertPath, statusPath string) tview.Primitive {
	view := tview.NewTextView().SetDynamicColors(true)
	view.SetBorder(true).SetTitle(" Intrusion Detection — live ")

	redraw := func() {
		report := idsMonitorReport(alertPath, statusPath, time.Now())
		app.QueueUpdateDraw(func() {
			view.Clear()
			fmt.Fprint(view, report)
		})
	}

	// Drawn once so the page is never briefly blank. The read happens off the
	// UI goroutine; only the update is queued back onto it.
	view.SetText(idsMonitorReport(alertPath, statusPath, time.Now()))
	go func() {
		for range time.Tick(idsRefresh) {
			redraw()
		}
	}()

	return view
}
