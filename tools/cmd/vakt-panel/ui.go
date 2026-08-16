package main

import (
	"fmt"
	"os"
	"regexp"
	"strconv"
	"strings"

	"github.com/gdamore/tcell/v2"
	"github.com/rivo/tview"
)

const (
	accent = "[#5fd7d7]"
	dim    = "[#767676]"
	plain  = "[#dadada]"
	ok     = "[#5faf5f]"
	warn   = "[#d7af5f]"
	bad    = "[#d75f5f]"
	off    = "[-]"
)

var (
	accentColor = tcell.NewHexColor(0x5fd7d7)
	dimColor    = tcell.NewHexColor(0x767676)
	plainColor  = tcell.NewHexColor(0xdadada)
	panelBG     = tcell.NewHexColor(0x121212)
	fieldBG     = tcell.NewHexColor(0x303030)
)

func applyTheme() {
	tview.Styles.PrimitiveBackgroundColor = panelBG
	tview.Styles.ContrastBackgroundColor = fieldBG
	tview.Styles.MoreContrastBackgroundColor = fieldBG
	tview.Styles.PrimaryTextColor = plainColor
	tview.Styles.SecondaryTextColor = accentColor
	tview.Styles.TertiaryTextColor = dimColor
	tview.Styles.InverseTextColor = accentColor
	tview.Styles.BorderColor = dimColor
	tview.Styles.TitleColor = accentColor
	tview.Styles.GraphicsColor = dimColor
}

type chrome struct {
	status *tview.TextView
	footer *tview.TextView
	pages  *tview.Pages
	layout *tview.Flex
}

func newChrome() *chrome {
	title := tview.NewTextView().
		SetDynamicColors(true).
		SetText(" " + accent + "[::b]VAKT OS" + off)

	status := tview.NewTextView().
		SetDynamicColors(true).
		SetTextAlign(tview.AlignRight)

	header := tview.NewFlex().
		AddItem(title, 10, 0, false).
		AddItem(status, 0, 1, false)

	footer := tview.NewTextView().SetDynamicColors(true)
	pages := tview.NewPages()

	layout := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(header, 1, 0, false).
		AddItem(rule(), 1, 0, false).
		AddItem(pages, 0, 1, true).
		AddItem(rule(), 1, 0, false).
		AddItem(footer, 1, 0, false)

	return &chrome{status: status, footer: footer, pages: pages, layout: layout}
}

func rule() *tview.Box {
	return tview.NewBox().SetDrawFunc(func(screen tcell.Screen, x, y, w, h int) (int, int, int, int) {
		style := tcell.StyleDefault.Foreground(fieldBG).Background(panelBG)
		for i := 0; i < w; i++ {
			screen.SetContent(x+i, y, '─', nil, style)
		}
		return x, y, w, h
	})
}

func (c *chrome) setStatus() {
	net := readKeyValueFile(netStatusFile)
	state := net["state"]
	colour := warn
	switch state {
	case "connected":
		colour = ok
	case "failed", "unconfigured":
		colour = bad
	case "":
		state = "unknown"
		colour = dim
	}

	alerts := alertCount()
	alertColour := ok
	if alerts > 0 {
		alertColour = bad
	}

	c.status.SetText(fmt.Sprintf("%snet%s %s%s%s   %salerts%s %s%d%s   %sup%s %s%s%s ",
		dim, off, colour, state, off,
		dim, off, alertColour, alerts, off,
		dim, off, plain, uptime(), off))
}

func (c *chrome) setKeys(local ...string) {
	parts := append([]string{}, local...)
	parts = append(parts, "?:keys", "q:quit")

	var rendered []string
	for _, part := range parts {
		k, label, _ := strings.Cut(part, ":")
		rendered = append(rendered, accent+k+off+" "+dim+label+off)
	}
	c.footer.SetText(" " + strings.Join(rendered, dim+"  ·  "+off))
}

func section(name string) string {
	return accent + "[::b]" + name + off
}

func row(label, value string, width int) string {
	return "  " + dim + pad(label, width) + off + plain + value + off
}

func pad(s string, width int) string {
	if len(s) >= width {
		return s + " "
	}
	return s + strings.Repeat(" ", width-len(s))
}

var tagPattern = regexp.MustCompile(`\[[a-zA-Z0-9_,;:#\-\.]*\]`)

func stripTags(s string) string { return tagPattern.ReplaceAllString(s, "") }

func rootMode() string {
	data, err := os.ReadFile("/proc/mounts")
	if err != nil {
		return "unknown"
	}
	for _, line := range strings.Split(string(data), "\n") {
		fields := strings.Fields(line)
		if len(fields) < 4 || fields[1] != "/" {
			continue
		}
		for _, opt := range strings.Split(fields[3], ",") {
			if opt == "ro" {
				return "read-only"
			}
			if opt == "rw" {
				return "writable"
			}
		}
	}
	return "unknown"
}

func mounted(path string) bool {
	data, err := os.ReadFile("/proc/mounts")
	if err != nil {
		return false
	}
	for _, line := range strings.Split(string(data), "\n") {
		fields := strings.Fields(line)
		if len(fields) >= 2 && fields[1] == path {
			return true
		}
	}
	return false
}

func uptime() string {
	data, err := os.ReadFile("/proc/uptime")
	if err != nil {
		return "?"
	}
	fields := strings.Fields(string(data))
	if len(fields) == 0 {
		return "?"
	}
	seconds, err := strconv.ParseFloat(fields[0], 64)
	if err != nil {
		return "?"
	}

	total := int(seconds)
	switch {
	case total < 60:
		return fmt.Sprintf("%ds", total)
	case total < 3600:
		return fmt.Sprintf("%dm", total/60)
	case total < 86400:
		return fmt.Sprintf("%dh %dm", total/3600, (total%3600)/60)
	default:
		return fmt.Sprintf("%dd %dh", total/86400, (total%86400)/3600)
	}
}

func alertCount() int {
	data, err := os.ReadFile(idsAlerts)
	if err != nil {
		return 0
	}
	count := 0
	for _, alert := range parseAlerts(string(data)) {
		if alert.Kind != "INFO" {
			count++
		}
	}
	return count
}
