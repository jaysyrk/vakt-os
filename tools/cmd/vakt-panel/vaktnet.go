package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

const (
	persistentNetConf = "/persistent/etc/vakt-net.conf"
	fallbackNetConf   = "/etc/vakt-net.conf"
	netStatusFile     = "/run/vakt-net.status"
	servicesStatus    = "/run/services.status"
	idsAlerts         = "/run/vakt-ids.alerts"
)

// netConfPath returns the persistent config location when the disk is mounted,
// so credentials survive a reboot, and the RAM-only path otherwise.
func netConfPath() string {
	if info, err := os.Stat("/persistent"); err == nil && info.IsDir() {
		return persistentNetConf
	}
	return fallbackNetConf
}

// writeNetConfig saves Wi-Fi credentials. vakt-net polls this file's mtime and
// reconnects on its own once it changes, so there is no daemon to signal.
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

	// The PSK is in here, so keep it readable by root only.
	return path, os.WriteFile(path, []byte(b.String()), 0600)
}

// readNetConfig loads the saved SSID and interface. The PSK is deliberately
// not returned - the form should not redisplay a stored password.
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

// readKeyValueFile parses the key=value status files written by vakt-net.
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

// netStatusReport renders /run/vakt-net.status for display.
func netStatusReport() string { return netStatusReportFrom(netStatusFile) }

func netStatusReportFrom(path string) string {
	status := readKeyValueFile(path)
	if len(status) == 0 {
		return "[yellow]vakt-net has not reported yet.[-]\n" +
			"The daemon writes " + path + " once it starts.\n"
	}

	color := "yellow"
	switch status["state"] {
	case "connected":
		color = "green"
	case "failed", "unconfigured":
		color = "red"
	}

	var b strings.Builder
	fmt.Fprintf(&b, "State:     [%s]%s[-]\n", color, status["state"])
	fmt.Fprintf(&b, "Interface: %s\n", status["interface"])
	if ssid := status["ssid"]; ssid != "" {
		fmt.Fprintf(&b, "SSID:      %s\n", ssid)
	}
	if ip := status["ip"]; ip != "" {
		fmt.Fprintf(&b, "Address:   [green]%s[-]\n", ip)
	}
	if detail := status["detail"]; detail != "" {
		fmt.Fprintf(&b, "Detail:    %s\n", detail)
	}
	return b.String()
}

// servicesReport renders the supervisor's summary from vakt-init.
func servicesReport() string { return servicesReportFrom(servicesStatus) }

func servicesReportFrom(path string) string {
	data, err := os.ReadFile(path)
	if err != nil {
		return "[yellow]No service report yet.[-]\n" +
			"vakt-init writes " + path + " once the supervisor starts.\n"
	}

	var b strings.Builder
	b.WriteString("[::b]SERVICE          STATE      PID     RST  READY[-]\n\n")
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

		color := "yellow"
		switch state {
		case "running":
			color = "green"
		case "failed":
			color = "red"
		}
		if pid == "" {
			pid = "-"
		}

		readyColor := "gray"
		switch readiness {
		case "ready":
			readyColor = "green"
		case "waiting":
			readyColor = "yellow"
		}

		fmt.Fprintf(&b, "%-16s [%s]%-10s[-] %-7s %-4s [%s]%s[-]\n",
			name, color, state, pid, restarts, readyColor, readiness)
		if detail != "" {
			fmt.Fprintf(&b, "  [gray]%s[-]\n", detail)
		}
	}
	return b.String()
}

// idsReport shows the most recent intrusion detection findings.
func idsReport(limit int) string { return idsReportFrom(idsAlerts, limit) }

func idsReportFrom(path string, limit int) string {
	data, err := os.ReadFile(path)
	if err != nil {
		return "[green]No alerts recorded.[-]\n" +
			"vakt-ids writes findings to " + path + ".\n"
	}

	lines := strings.Split(strings.TrimRight(string(data), "\n"), "\n")
	if len(lines) > limit {
		lines = lines[len(lines)-limit:]
	}

	var b strings.Builder
	fmt.Fprintf(&b, "[::b]Most recent %d alert(s):[-]\n\n", len(lines))
	for _, line := range lines {
		fields := strings.SplitN(line, "\t", 3)
		if len(fields) < 3 {
			b.WriteString(line + "\n")
			continue
		}
		timestamp, kind, detail := fields[0], fields[1], fields[2]

		color := "red"
		if kind == "INFO" {
			color = "gray"
		}
		fmt.Fprintf(&b, "[gray]%s[-] [%s]%-12s[-] %s\n", timestamp, color, kind, detail)
	}
	return b.String()
}
