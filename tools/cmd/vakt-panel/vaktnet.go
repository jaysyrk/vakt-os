package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"vakt-os/tools/internal/durable"
)

const (
	persistentNetConf = "/persistent/etc/vakt-net.conf"
	fallbackNetConf   = "/etc/vakt-net.conf"
	netStatusFile     = "/run/vakt-net.status"
	servicesStatus    = "/run/services.status"
	idsAlerts         = "/run/vakt-ids.alerts"
)

// The persistent path when the disk is mounted, the RAM-only one otherwise.
func netConfPath() string {
	if info, err := os.Stat("/persistent"); err == nil && info.IsDir() {
		return persistentNetConf
	}
	return fallbackNetConf
}

// vakt-net polls this file's mtime, so there is no daemon to signal.
func writeNetConfig(ssid, psk, iface string) (string, error) {
	path := netConfPath()
	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return path, err
	}

	var b strings.Builder
	b.WriteString("# Written by vakt-panel\n")
	b.WriteString(fmt.Sprintf("ssid=%s\n", ssid))
	b.WriteString(fmt.Sprintf("psk=%s\n", psk))
	b.WriteString(fmt.Sprintf("interface=%s\n", iface))

	// In place, not temp-then-rename: Landlock rules are keyed on the inode
	// vakt-net locked, and a rename would leave one it cannot reach.
	return path, durable.WriteInPlace(path, []byte(b.String()), 0600)
}

// The PSK is deliberately not returned; the form must not redisplay it.
func readNetConfig() (ssid, iface string) {
	iface = "wlan0"
	data, err := os.ReadFile(netConfPath())
	if err != nil {
		return "", iface
	}
	for _, line := range strings.Split(string(data), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		key, value, found := strings.Cut(line, "=")
		if !found {
			continue
		}
		switch strings.ToLower(strings.TrimSpace(key)) {
		case "ssid":
			ssid = strings.TrimSpace(value)
		case "interface", "iface":
			iface = strings.TrimSpace(value)
		}
	}
	return ssid, iface
}

func readKeyValueFile(path string) map[string]string {
	values := map[string]string{}
	data, err := os.ReadFile(path)
	if err != nil {
		return values
	}
	for _, line := range strings.Split(string(data), "\n") {
		if key, value, found := strings.Cut(line, "="); found {
			values[strings.TrimSpace(key)] = strings.TrimSpace(value)
		}
	}
	return values
}

func netStatusReport() string { return netStatusReportFrom(netStatusFile) }

func netStatusReportFrom(path string) string {
	status := readKeyValueFile(path)
	if len(status) == 0 {
		return " " + warn + "vakt-net has not reported yet." + off + "\n" +
			" " + dim + "The daemon writes " + path + " once it starts." + off + "\n"
	}

	colour := warn
	switch status["state"] {
	case "connected":
		colour = ok
	case "failed", "unconfigured":
		colour = bad
	}

	var b strings.Builder
	fmt.Fprintf(&b, "  %s%s%s%s%s%s\n", dim, pad("state", 11), off, colour, status["state"], off)
	for _, field := range []struct{ label, key string }{
		{"interface", "interface"},
		{"ssid", "ssid"},
		{"address", "ip"},
		{"detail", "detail"},
	} {
		if value := status[field.key]; value != "" {
			b.WriteString(row(field.label, value, 11) + "\n")
		}
	}
	return b.String()
}

func servicesReport() string { return servicesReportFrom(servicesStatus) }

func servicesReportFrom(path string) string {
	data, err := os.ReadFile(path)
	if err != nil {
		return " " + warn + "No service report yet." + off + "\n" +
			" " + dim + "vakt-init writes " + path + " once the supervisor starts." + off + "\n"
	}

	var b strings.Builder
	fmt.Fprintf(&b, "  %s%s%s%s%s%s\n\n", dim,
		pad("service", 17), pad("state", 11), pad("pid", 8), pad("rst", 5), "ready"+off)
	for _, line := range strings.Split(strings.TrimRight(string(data), "\n"), "\n") {
		if line == "" {
			continue
		}
		// name, state, pid, restarts, readiness, detail. Older supervisors
		// wrote five fields; pad rather than drop the line.
		fields := strings.Split(line, "\t")
		for len(fields) < 6 {
			fields = append(fields, "")
		}
		name, state, pid, restarts := fields[0], fields[1], fields[2], fields[3]
		readiness, detail := fields[4], fields[5]

		colour := warn
		switch state {
		case "running":
			colour = ok
		case "failed":
			colour = bad
		}
		if pid == "" {
			pid = "-"
		}

		readyColour := dim
		switch readiness {
		case "ready":
			readyColour = ok
		case "waiting":
			readyColour = warn
		}

		fmt.Fprintf(&b, "  %s%s%s%s%s%s%s%s%s%s%s\n",
			plain, pad(name, 17), off,
			colour, pad(state, 11), off,
			plain, pad(pid, 8)+pad(restarts, 5), off,
			readyColour, readiness+off)
		if detail != "" {
			fmt.Fprintf(&b, "  %s%s%s\n", dim, "  "+detail, off)
		}
	}
	return b.String()
}

func idsReport(limit int) string { return idsReportFrom(idsAlerts, limit) }

func idsReportFrom(path string, limit int) string {
	data, err := os.ReadFile(path)
	if err != nil {
		// Only a missing file means "nothing reported".
		if !os.IsNotExist(err) {
			return "  " + bad + "Cannot read " + path + ": " + err.Error() + off + "\n\n" +
				"  " + plain + "This is not an all-clear. vakt-ids may be recording findings\n" +
				"  this page cannot see. Check the file from a root shell." + off + "\n"
		}
		return noAlerts(path)
	}

	if len(strings.TrimSpace(string(data))) == 0 {
		return noAlerts(path)
	}

	lines := strings.Split(strings.TrimRight(string(data), "\n"), "\n")
	if len(lines) > limit {
		lines = lines[len(lines)-limit:]
	}

	var b strings.Builder
	fmt.Fprintf(&b, "  %sMost recent %d alert(s)%s\n\n", dim, len(lines), off)
	for _, line := range lines {
		fields := strings.SplitN(line, "\t", 3)
		if len(fields) < 3 {
			b.WriteString("  " + line + "\n")
			continue
		}
		timestamp, kind, detail := fields[0], fields[1], fields[2]

		colour := bad
		if kind == "INFO" {
			colour = dim
		}
		fmt.Fprintf(&b, "  %s%s%s %s%s%s %s%s%s\n",
			dim, timestamp, off, colour, pad(kind, 12), off, plain, detail, off)
	}
	return b.String()
}

func noAlerts(path string) string {
	return "  " + ok + "No alerts recorded." + off + "\n" +
		"  " + dim + "vakt-ids writes findings to " + path + "." + off + "\n"
}
