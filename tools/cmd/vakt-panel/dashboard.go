package main

import (
	"fmt"
	"os"
	"sort"
	"strings"
)

const gutter = 40

func dashboardText() string {
	var b strings.Builder
	b.WriteString("\n")
	for _, line := range sideBySide(networkBlock(), systemBlock()) {
		b.WriteString(" " + line + "\n")
	}
	b.WriteString("\n")
	for _, line := range sideBySide(servicesBlock(), integrityBlock()) {
		b.WriteString(" " + line + "\n")
	}
	b.WriteString("\n")
	b.WriteString(navStrip())
	return b.String()
}

func networkBlock() []string {
	status := readKeyValueFile(netStatusFile)
	if len(status) == 0 {
		return []string{section("NETWORK"), "  " + dim + "vakt-net has not reported yet" + off}
	}

	state := status["state"]
	colour := warn
	switch state {
	case "connected":
		colour = ok
	case "failed", "unconfigured":
		colour = bad
	}

	lines := []string{
		section("NETWORK"),
		"  " + dim + pad("state", 11) + off + colour + state + off,
	}
	if ssid := status["ssid"]; ssid != "" {
		lines = append(lines, row("ssid", shorten(ssid, 22), 11))
	}
	if ip := status["ip"]; ip != "" {
		lines = append(lines, row("address", ip, 11))
	}
	if iface := status["interface"]; iface != "" {
		lines = append(lines, row("interface", iface, 11))
	}
	if detail := status["detail"]; detail != "" && state != "connected" {
		lines = append(lines, "  "+dim+shorten(detail, 34)+off)
	}
	return lines
}

func systemBlock() []string {
	persistent := bad + "not mounted" + off
	if mounted("/persistent") {
		persistent = ok + "mounted" + off
	}

	mode := rootMode()
	modeColour := warn
	if mode == "read-only" {
		modeColour = ok
	}

	return []string{
		section("SYSTEM"),
		row("uptime", uptime(), 12),
		"  " + dim + pad("root", 12) + off + modeColour + mode + off,
		"  " + dim + pad("/persistent", 12) + off + persistent,
	}
}

func servicesBlock() []string {
	data, err := os.ReadFile(servicesStatus)
	if err != nil {
		return []string{section("SERVICES"), "  " + dim + "no supervisor report" + off}
	}

	lines := []string{section("SERVICES")}
	for _, line := range strings.Split(strings.TrimRight(string(data), "\n"), "\n") {
		fields := strings.Split(line, "\t")
		if len(fields) < 5 || fields[0] == "" {
			continue
		}
		name, state, readiness := fields[0], fields[1], fields[4]

		label, colour := state, bad
		switch {
		case state == "running" && readiness == "ready":
			label, colour = "ready", ok
		case state == "running":
			label, colour = readiness, warn
		}
		if label == "" {
			label = state
		}
		lines = append(lines, "  "+dim+pad(name, 12)+off+colour+label+off)
	}
	if len(lines) == 1 {
		lines = append(lines, "  "+dim+"no services reported"+off)
	}
	return lines
}

func integrityBlock() []string {
	data, err := os.ReadFile(idsAlerts)
	if err != nil {
		if !os.IsNotExist(err) {
			return []string{section("INTEGRITY"), "  " + bad + "cannot read findings" + off}
		}
		return []string{section("INTEGRITY"), "  " + ok + "no findings" + off}
	}

	alerts := parseAlerts(string(data))
	counts := countByKind(alerts)

	var kinds []string
	for kind, n := range counts {
		if kind != "INFO" && n > 0 {
			kinds = append(kinds, fmt.Sprintf("%s %d", strings.ToLower(kind), n))
		}
	}
	sort.Strings(kinds)

	lines := []string{section("INTEGRITY")}
	if len(kinds) == 0 {
		lines = append(lines, "  "+ok+"no findings"+off)
	} else {
		lines = append(lines, "  "+bad+shorten(strings.Join(kinds, "  "), 34)+off)
	}

	for i := len(alerts) - 1; i >= 0; i-- {
		if alerts[i].Kind == "INFO" {
			continue
		}
		stamp := "        "
		if alerts[i].HasTime {
			stamp = alerts[i].When.Local().Format("15:04:05")
		}
		lines = append(lines, "  "+dim+stamp+" "+off+plain+shorten(alerts[i].Detail, 24)+off)
		break
	}
	return lines
}

func navStrip() string {
	entries := [][2]string{
		{"d", "home"}, {"n", "network"}, {"w", "wi-fi"}, {"i", "intrusion"},
		{"p", "packages"}, {"s", "audit"}, {"v", "services"}, {"l", "lock"},
		{"g", "graphical"}, {"o", "power"}, {"q", "quit"},
	}

	var b strings.Builder
	for i, entry := range entries {
		if i%5 == 0 {
			if i > 0 {
				b.WriteString("\n")
			}
			b.WriteString("   ")
		}
		b.WriteString(accent + entry[0] + off + " " + dim + pad(entry[1], 13) + off)
	}
	return b.String() + "\n"
}

// Pads to the gutter by printed width, not byte length: colour tags occupy no
// columns.
func sideBySide(left, right []string) []string {
	height := len(left)
	if len(right) > height {
		height = len(right)
	}

	out := make([]string, 0, height)
	for i := 0; i < height; i++ {
		var l, r string
		if i < len(left) {
			l = left[i]
		}
		if i < len(right) {
			r = right[i]
		}
		if r == "" {
			out = append(out, l)
			continue
		}
		width := len([]rune(stripTags(l)))
		if width < gutter {
			l += strings.Repeat(" ", gutter-width)
		}
		out = append(out, l+r)
	}
	return out
}

func shorten(s string, width int) string {
	runes := []rune(s)
	if len(runes) <= width {
		return s
	}
	if width <= 1 {
		return string(runes[:width])
	}
	return string(runes[:width-1]) + "…"
}
