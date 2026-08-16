package main

import (
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/gdamore/tcell/v2"
	"github.com/rivo/tview"
)

const tick = 2 * time.Second

type destination struct {
	page  string
	focus tview.Primitive
	open  func()
}

func main() {
	applyTheme()

	app := tview.NewApplication()
	ui := newChrome()

	newOutputView := func() *tview.TextView {
		return tview.NewTextView().
			SetDynamicColors(true).
			SetScrollable(true).
			SetChangedFunc(func() { app.Draw() })
	}

	runInto := func(view *tview.TextView, name string, args ...string) {
		view.Clear()
		fmt.Fprintf(view, "%srunning %s…%s\n\n", dim, name, off)
		go func() {
			out, err := exec.Command(name, args...).CombinedOutput()
			if err != nil {
				fmt.Fprintf(view, "%s%v%s\n", bad, err, off)
			}
			fmt.Fprint(view, string(out))
		}()
	}

	home := tview.NewTextView().SetDynamicColors(true)
	refreshHome := func() { home.SetText(dashboardText()) }
	refreshHome()

	auditView := newOutputView()

	pkgView := newOutputView()
	pkgForm := tview.NewForm().
		AddInputField("Package", "", 24, nil, nil).
		AddInputField("Repository", readRepoURL(), 34, nil, nil)
	styleForm(pkgForm)

	pkgField := func(i int) string {
		return pkgForm.GetFormItem(i).(*tview.InputField).GetText()
	}
	pkgForm.AddButton("Install", func() {
		name := pkgField(0)
		if name == "" {
			pkgView.Clear()
			fmt.Fprint(pkgView, bad+"Enter a package name first."+off)
			return
		}
		runInto(pkgView, "zrpkg", "install", name)
	})
	pkgForm.AddButton("List", func() { runInto(pkgView, "zrpkg", "update") })
	pkgForm.AddButton("Save repository", func() {
		pkgView.Clear()
		url, path, err := writeRepoURL(pkgField(1))
		if err != nil {
			fmt.Fprintf(pkgView, "%s%v%s\n", bad, err, off)
			return
		}
		pkgForm.GetFormItem(1).(*tview.InputField).SetText(url)
		fmt.Fprintf(pkgView, "%sRepository set to %s%s\n%s%s%s\n", ok, url, off, dim, path, off)
		if strings.HasPrefix(url, "http://") {
			fmt.Fprintf(pkgView, "\n%sPlain HTTP. Signatures still protect what you install,\nbut anyone on the path can see what you install.%s\n", warn, off)
		}
	})

	pkgPage := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(pkgForm, 9, 0, true).
		AddItem(pkgView, 0, 1, false)

	netView := newOutputView()
	refreshNet := func() {
		netView.Clear()
		fmt.Fprint(netView, netStatusReport())
	}
	netAddrBtn := button("Show interfaces (ip addr)", func() { runInto(netView, "ip", "addr") })

	netPage := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(netView, 0, 1, false).
		AddItem(bottomBar(netAddrBtn, 26), 1, 0, true)

	wifi, wifiFocus, refreshWifi := wifiPage(app)

	servicesView := newOutputView()
	refreshServices := func() {
		servicesView.Clear()
		fmt.Fprint(servicesView, servicesReport())
	}
	servicesLogBtn := button("vakt-net log", func() { runInto(servicesView, "cat", "/run/vakt-net.log") })

	servicesPage := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(servicesView, 0, 1, false).
		AddItem(bottomBar(servicesLogBtn, 14), 1, 0, true)

	ids := idsMonitor(app, idsAlerts, servicesStatus)

	recordView := newOutputView()
	refreshRecord := func() {
		recordView.Clear()
		fmt.Fprint(recordView, idsReport(200))
	}

	lock, lockFocus := lockPage()
	power, powerFocus := powerPage()

	for _, entry := range []struct {
		name  string
		title string
		body  tview.Primitive
	}{
		{"home", "", home},
		{"audit", "SECURITY AUDIT", auditView},
		{"packages", "PACKAGES", pkgPage},
		{"network", "NETWORK", netPage},
		{"wifi", "", wifi},
		{"services", "SERVICES", servicesPage},
		{"ids", "INTRUSION DETECTION", ids},
		{"record", "INTEGRITY RECORD", recordView},
		{"lock", "PANEL LOCK", lock},
		{"power", "POWER", power},
		{"help", "KEYS", helpPage()},
	} {
		ui.pages.AddPage(entry.name, framed(entry.title, entry.body), true, entry.name == "home")
	}

	routes := map[rune]destination{
		'd': {page: "home", focus: home, open: refreshHome},
		's': {page: "audit", focus: auditView, open: func() { runInto(auditView, "vakt-audit") }},
		'p': {page: "packages", focus: pkgForm},
		'n': {page: "network", focus: netAddrBtn, open: refreshNet},
		'w': {page: "wifi", focus: wifiFocus, open: refreshWifi},
		'v': {page: "services", focus: servicesLogBtn, open: refreshServices},
		'i': {page: "ids", focus: ids},
		'f': {page: "record", focus: recordView, open: refreshRecord},
		'l': {page: "lock", focus: lockFocus()},
		'o': {page: "power", focus: powerFocus},
		'?': {page: "help", focus: nil},
	}

	// Esc is advertised everywhere: a focused field swallows the letters.
	keys := map[string][]string{
		"home":     {},
		"audit":    {"esc:back", "s:rerun"},
		"packages": {"esc:back", "tab:move"},
		"network":  {"esc:back", "enter:ip addr"},
		"wifi":     {"esc:back", "enter:select", "w:rescan"},
		"services": {"esc:back", "enter:log"},
		"ids":      {"esc:back", "f:full record"},
		"record":   {"esc:back", "i:live"},
		"lock":     {"esc:back", "tab:move"},
		"power":    {"esc:back", "tab:move"},
		"help":     {"esc:back"},
	}

	goTo := func(r rune) {
		to, known := routes[r]
		if !known {
			return
		}
		if to.open != nil {
			to.open()
		}
		// Which fields the lock form has is what that page changes.
		if r == 'l' {
			to.focus = lockFocus()
		}
		ui.pages.SwitchToPage(to.page)
		ui.setKeys(keys[to.page]...)
		if to.focus != nil {
			app.SetFocus(to.focus)
		}
	}
	ui.setKeys()
	ui.setStatus()

	unlocked := false
	app.SetInputCapture(func(event *tcell.EventKey) *tcell.EventKey {
		if !unlocked {
			return event
		}
		if event.Key() == tcell.KeyEsc {
			goTo('d')
			return nil
		}
		if event.Key() != tcell.KeyRune || typing(app) {
			return event
		}
		switch event.Rune() {
		case 'q':
			app.Stop()
			return nil
		case 'g':
			launchCompositor(app, ui, home)
			return nil
		}
		if _, known := routes[event.Rune()]; known {
			goTo(event.Rune())
			return nil
		}
		return event
	})

	go func() {
		for range time.Tick(tick) {
			name, _ := ui.pages.GetFrontPage()
			// Boot messages printed to the console after the panel first drew
			// sit in cells tcell believes it already owns, so they survive
			// every redraw. Sync repaints all of them.
			app.Sync()
			app.QueueUpdateDraw(func() {
				ui.setStatus()
				switch name {
				case "home":
					refreshHome()
				case "services":
					refreshServices()
				case "network":
					refreshNet()
				case "wifi":
					refreshWifi()
				}
			})
		}
	}()

	app.EnableMouse(true)
	gate := authGateRoot(app, ui.layout, home, func() { unlocked = true })
	// tcell only repaints cells it has content for; boot messages remain.
	fmt.Print("\033[2J\033[H")

	if err := app.SetRoot(gate, true).Run(); err != nil {
		panic(err)
	}
}

// Without this, a password containing "q" quits the panel.
func typing(app *tview.Application) bool {
	switch app.GetFocus().(type) {
	case *tview.InputField, *tview.TextArea:
		return true
	}
	return false
}

func button(label string, selected func()) *tview.Button {
	b := tview.NewButton(label).SetSelectedFunc(selected)
	b.SetLabelColor(accentColor).SetBackgroundColor(fieldBG)
	return b
}

// A tview Button centres its label across whatever width it is given.
func bottomBar(b *tview.Button, width int) tview.Primitive {
	return tview.NewFlex().
		AddItem(tview.NewBox(), 1, 0, false).
		AddItem(b, width, 0, true).
		AddItem(tview.NewBox(), 0, 1, false)
}

func styleForm(form *tview.Form) {
	form.SetFieldBackgroundColor(fieldBG).
		SetLabelColor(dimColor).
		SetFieldTextColor(plainColor).
		SetButtonBackgroundColor(fieldBG).
		SetButtonTextColor(accentColor)
}

func framed(title string, body tview.Primitive) tview.Primitive {
	if title == "" {
		return body
	}
	heading := tview.NewTextView().SetDynamicColors(true).SetText(" " + section(title))
	return tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(heading, 1, 0, false).
		AddItem(tview.NewBox(), 1, 0, false).
		AddItem(body, 0, 1, true)
}

func helpPage() tview.Primitive {
	entries := [][2]string{
		{"d", "Dashboard - link state, services, findings"},
		{"n", "Network - addresses and interface detail"},
		{"w", "Wi-Fi - pick a network and connect"},
		{"i", "Intrusion detection - live findings"},
		{"f", "Integrity record - every finding still on file"},
		{"s", "Security audit - run vakt-audit"},
		{"p", "Packages - install software with zrpkg"},
		{"v", "Services - supervisor state and logs"},
		{"l", "Panel lock - set, change or remove the PIN"},
		{"g", "Graphical mode - hand the console to the compositor"},
		{"o", "Power - shut down or restart"},
		{"q", "Quit to a shell"},
		{"esc", "Back to the dashboard"},
	}

	var b strings.Builder
	b.WriteString("\n")
	for _, entry := range entries {
		fmt.Fprintf(&b, "   %s%s%s  %s%s%s\n", accent, pad(entry[0], 5), off, plain, entry[1], off)
	}
	fmt.Fprintf(&b, "\n   %sShortcuts are ignored while a text field has focus.%s\n", dim, off)

	return tview.NewTextView().SetDynamicColors(true).SetText(b.String())
}

// Rebuilt on every visit: the extra field and button exist only once a PIN does.
func lockPage() (tview.Primitive, func() tview.Primitive) {
	status := tview.NewTextView().SetDynamicColors(true)
	result := tview.NewTextView().SetDynamicColors(true).SetWordWrap(true)
	form := tview.NewForm()
	styleForm(form)

	var rebuild func()
	rebuild = func() {
		form.Clear(true)
		protected := hasPIN()
		if protected {
			status.SetText(" " + ok + "This panel is PIN protected." + off)
			form.AddPasswordField("Current PIN", "", 20, '*', nil)
		} else {
			status.SetText(" " + bad + "No PIN is set. Anyone with console access has full control." + off)
		}
		form.AddPasswordField("New PIN", "", 20, '*', nil)
		form.AddPasswordField("Confirm New PIN", "", 20, '*', nil)

		field := func(label string) string {
			item := form.GetFormItemByLabel(label)
			if item == nil {
				return ""
			}
			return item.(*tview.InputField).GetText()
		}

		save := func() {
			result.Clear()
			if protected && !verifyPIN(field("Current PIN")) {
				fmt.Fprint(result, bad+"Current PIN is incorrect."+off)
				return
			}
			newPIN := field("New PIN")
			if newPIN == "" {
				fmt.Fprint(result, bad+"New PIN cannot be empty."+off)
				return
			}
			if newPIN != field("Confirm New PIN") {
				fmt.Fprint(result, bad+"New PIN and confirmation do not match."+off)
				return
			}
			if err := setPIN(newPIN); err != nil {
				fmt.Fprintf(result, "%sCould not save PIN: %v%s", bad, err, off)
				return
			}
			fmt.Fprint(result, ok+"PIN saved."+off)
			rebuild()
		}
		form.AddButton("Set / change PIN", save)
		submitOnEnter(form, save)

		if protected {
			form.AddButton("Remove PIN", func() {
				result.Clear()
				if !verifyPIN(field("Current PIN")) {
					fmt.Fprint(result, bad+"Current PIN is incorrect."+off)
					return
				}
				if err := removePIN(); err != nil {
					fmt.Fprintf(result, "%sCould not remove PIN: %v%s", bad, err, off)
					return
				}
				fmt.Fprint(result, warn+"PIN removed. This panel is no longer protected."+off)
				rebuild()
			})
		}
	}
	rebuild()

	page := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(status, 2, 0, false).
		AddItem(form, 9, 0, true).
		AddItem(result, 0, 1, false)

	focus := func() tview.Primitive {
		rebuild()
		return form
	}
	return page, focus
}

// Through vakt-init's socket: the panel is unprivileged and cannot signal PID 1.
func powerPage() (tview.Primitive, tview.Primitive) {
	result := tview.NewTextView().SetDynamicColors(true).SetWordWrap(true)

	request := func(verb, describe string) {
		result.Clear()
		if err := requestShutdown(verb); err != nil {
			fmt.Fprintf(result, "%s%s failed: %v%s\n\n%sFrom a root shell, busybox 'poweroff' and 'reboot' signal vakt-init directly.%s",
				bad, describe, err, off, dim, off)
			return
		}
		fmt.Fprintf(result, "%s%s requested.%s\n\n%svakt-init is stopping services, flushing disks and unmounting /persistent.%s",
			warn, describe, off, dim, off)
	}

	powerOff := button("Power off", func() { request("poweroff", "Power off") })
	reboot := button("Reboot", func() { request("reboot", "Reboot") })

	buttons := tview.NewFlex().
		AddItem(tview.NewBox(), 1, 0, false).
		AddItem(powerOff, 16, 0, true).
		AddItem(tview.NewBox(), 2, 0, false).
		AddItem(reboot, 16, 0, false).
		AddItem(tview.NewBox(), 0, 1, false)

	page := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(buttons, 1, 0, true).
		AddItem(tview.NewBox(), 1, 0, false).
		AddItem(result, 0, 1, false)

	return page, powerOff
}

// Suspend is what makes this safe: tview releases the terminal and stops drawing.
func launchCompositor(app *tview.Application, ui *chrome, report *tview.TextView) {
	ui.pages.SwitchToPage("home")

	var runErr error
	suspended := app.Suspend(func() {
		fmt.Print("\033[2J\033[H")
		fmt.Println("Switching to graphical mode. The compositor renders to /dev/fb0.")

		cmd := exec.Command("vakt-compositor")
		cmd.Stdin = os.Stdin
		cmd.Stdout = os.Stdout
		cmd.Stderr = os.Stderr
		runErr = cmd.Run()

		fmt.Print("\033[2J\033[H")
	})

	report.Clear()
	switch {
	case !suspended:
		fmt.Fprint(report, "\n  "+bad+"Could not suspend the panel; graphical mode unavailable."+off)
	case runErr != nil:
		fmt.Fprintf(report, "\n  %svakt-compositor exited with an error: %v%s\n\n  %sThis usually means /dev/fb0 is missing - the kernel needs a\n  framebuffer console for graphical mode to work.%s",
			bad, runErr, off, dim, off)
	default:
		report.SetText(dashboardText())
	}
}
