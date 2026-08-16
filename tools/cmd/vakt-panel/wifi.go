package main

import (
	"fmt"
	"os"
	"sort"
	"strconv"
	"strings"

	"github.com/gdamore/tcell/v2"
	"github.com/rivo/tview"
)

// Written by vakt-net: ssid, signal in dBm, frequency in MHz, security.
const scanFile = "/run/vakt-net.scan"

type network struct {
	SSID     string
	Signal   int
	Freq     int
	Security string
}

func (n network) band() string {
	if n.Freq >= 4900 {
		return "5GHz"
	}
	return "2.4GHz"
}

func (n network) bars() string {
	level := (n.Signal + 90) / 10
	if level < 0 {
		level = 0
	}
	if level > 4 {
		level = 4
	}
	return strings.Repeat("█", level) + strings.Repeat("░", 4-level)
}

func parseScan(text string) []network {
	strongest := map[string]network{}
	var order []string

	for _, line := range strings.Split(text, "\n") {
		fields := strings.Split(strings.TrimRight(line, "\r"), "\t")
		if len(fields) < 4 {
			continue
		}
		ssid := strings.TrimSpace(fields[0])
		if ssid == "" {
			continue
		}
		signal, err := strconv.Atoi(strings.TrimSpace(fields[1]))
		if err != nil {
			continue
		}
		freq, err := strconv.Atoi(strings.TrimSpace(fields[2]))
		if err != nil {
			continue
		}

		found := network{SSID: ssid, Signal: signal, Freq: freq, Security: strings.TrimSpace(fields[3])}
		if seen, ok := strongest[ssid]; ok {
			if found.Signal > seen.Signal {
				strongest[ssid] = found
			}
			continue
		}
		strongest[ssid] = found
		order = append(order, ssid)
	}

	networks := make([]network, 0, len(order))
	for _, ssid := range order {
		networks = append(networks, strongest[ssid])
	}
	sort.SliceStable(networks, func(i, j int) bool {
		return networks[i].Signal > networks[j].Signal
	})
	return networks
}

func readScan() []network {
	data, err := os.ReadFile(scanFile)
	if err != nil {
		return nil
	}
	return parseScan(string(data))
}

// A picker rather than a text box: a mistyped SSID reports only as "did not
// associate".
func wifiPage(app *tview.Application) (tview.Primitive, tview.Primitive, func()) {
	savedSSID, savedIface := readNetConfig()

	result := tview.NewTextView().SetDynamicColors(true).SetWordWrap(true)

	form := tview.NewForm().
		AddInputField("SSID", savedSSID, 24, nil, nil).
		AddPasswordField("Password", "", 24, '*', nil).
		AddInputField("Interface", savedIface, 12, nil, nil)
	form.SetFieldBackgroundColor(fieldBG).
		SetLabelColor(dimColor).
		SetFieldTextColor(plainColor).
		SetButtonBackgroundColor(fieldBG).
		SetButtonTextColor(accentColor)

	field := func(i int) string {
		return form.GetFormItem(i).(*tview.InputField).GetText()
	}

	connect := func() {
		ssid, psk, iface := field(0), field(1), field(2)
		result.Clear()
		if ssid == "" {
			fmt.Fprint(result, bad+"Pick a network, or type an SSID."+off)
			return
		}
		if iface == "" {
			iface = "wlan0"
		}
		path, err := writeNetConfig(ssid, psk, iface)
		if err != nil {
			fmt.Fprintf(result, "%sCould not write %s: %v%s", bad, path, err, off)
			return
		}
		fmt.Fprintf(result, "%sSaved.%s %svakt-net picks this up within a second; watch the\nheader for the link state.%s", ok, off, dim, off)
	}
	form.AddButton("Connect", connect)
	submitOnEnter(form, connect)

	table := tview.NewTable().SetSelectable(true, false).SetFixed(1, 0)
	table.SetSelectedStyle(tcell.StyleDefault.Foreground(accentColor).Background(fieldBG))
	table.SetBorderPadding(0, 0, 1, 0)

	var found []network
	refresh := func() {
		found = readScan()
		table.Clear()

		header := []string{"", "network", "band", "security"}
		for col, text := range header {
			table.SetCell(0, col, tview.NewTableCell(text).
				SetTextColor(dimColor).
				SetSelectable(false))
		}

		if len(found) == 0 {
			table.SetCell(1, 1, tview.NewTableCell("no networks in range").
				SetTextColor(dimColor).
				SetSelectable(false))
			return
		}
		for i, n := range found {
			table.SetCell(i+1, 0, tview.NewTableCell(n.bars()).SetTextColor(accentColor))
			table.SetCell(i+1, 1, tview.NewTableCell(shorten(n.SSID, 22)).SetTextColor(plainColor))
			table.SetCell(i+1, 2, tview.NewTableCell(n.band()).SetTextColor(dimColor))
			table.SetCell(i+1, 3, tview.NewTableCell(n.Security).SetTextColor(dimColor))
		}
		table.Select(1, 0)
	}
	refresh()

	table.SetSelectedFunc(func(rowIndex, _ int) {
		if rowIndex-1 < 0 || rowIndex-1 >= len(found) {
			return
		}
		chosen := found[rowIndex-1]
		form.GetFormItem(0).(*tview.InputField).SetText(chosen.SSID)
		result.Clear()
		fmt.Fprintf(result, "%s%s%s %s%s · %s%s", plain, chosen.SSID, off, dim, chosen.band(), chosen.Security, off)
		app.SetFocus(form.GetFormItem(1))
	})

	heading := func(text string) *tview.TextView {
		return tview.NewTextView().SetDynamicColors(true).SetText(" " + section(text))
	}

	left := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(heading("NETWORKS"), 1, 0, false).
		AddItem(table, 0, 1, true)

	right := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(heading("CONNECT"), 1, 0, false).
		AddItem(form, 0, 1, false)

	columns := tview.NewFlex().
		AddItem(left, 0, 1, true).
		AddItem(right, 36, 0, false)

	page := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(columns, 0, 1, true).
		AddItem(result, 3, 0, false)

	return page, table, refresh
}
