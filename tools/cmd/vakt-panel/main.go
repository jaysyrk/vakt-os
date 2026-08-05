package main

import (
	"fmt"
	"os"
	"os/exec"

	"github.com/gdamore/tcell/v2"
	"github.com/rivo/tview"
)

func main() {
	app := tview.NewApplication()
	pages := tview.NewPages()

	// newOutputView builds the scrollable output pane used by most pages.
	newOutputView := func() *tview.TextView {
		view := tview.NewTextView().
			SetDynamicColors(true).
			SetScrollable(true).
			SetChangedFunc(func() {
				app.Draw()
			})
		view.SetBorder(true).SetTitle(" Output ")
		return view
	}

	// runInto executes a command and streams its combined output into view.
	runInto := func(view *tview.TextView, name string, args ...string) {
		view.Clear()
		fmt.Fprintf(view, "[yellow]Running %s...[-]\n", name)
		go func() {
			out, err := exec.Command(name, args...).CombinedOutput()
			if err != nil {
				fmt.Fprintf(view, "[red]Error: %v[-]\n", err)
			}
			fmt.Fprint(view, string(out))
		}()
	}

	// --- 1. Dashboard ---
	dashboardText := tview.NewTextView().
		SetDynamicColors(true).
		SetText("[green]System Status: Online[-]\n\n" +
			"Welcome to Vakt OS Security Appliance.\n\n" +
			"Use [yellow]Arrow Keys[-] to navigate the menu.\n" +
			"Press [yellow]Enter[-] to select.\n" +
			"Press [yellow]ESC[-] to return focus to the main menu at any time.\n")
	dashboardText.SetBorder(true).SetTitle(" Dashboard ")
	pages.AddPage("Dashboard", dashboardText, true, true)

	// --- 2. Vakt Audit ---
	auditView := newOutputView()
	runAuditBtn := tview.NewButton("Start Scan").SetSelectedFunc(func() {
		runInto(auditView, "vakt-audit")
	})

	auditFlex := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(runAuditBtn, 3, 1, true).
		AddItem(auditView, 0, 4, false)

	pages.AddPage("Audit", auditFlex, true, false)

	// --- 3. ZRPKG Manager ---
	pkgView := newOutputView()
	pkgInput := tview.NewInputField().
		SetLabel(" Package: ").
		SetFieldWidth(24)

	listPkgBtn := tview.NewButton("List Available").SetSelectedFunc(func() {
		runInto(pkgView, "zrpkg", "update")
	})
	installPkgBtn := tview.NewButton("Install").SetSelectedFunc(func() {
		name := pkgInput.GetText()
		if name == "" {
			pkgView.Clear()
			fmt.Fprint(pkgView, "[red]Enter a package name first.[-]\n")
			return
		}
		runInto(pkgView, "zrpkg", "install", name)
	})

	pkgControls := tview.NewFlex().
		AddItem(pkgInput, 36, 0, true).
		AddItem(installPkgBtn, 0, 1, false).
		AddItem(listPkgBtn, 0, 1, false)

	pkgFlex := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(pkgControls, 3, 1, true).
		AddItem(pkgView, 0, 4, false)

	pages.AddPage("Packages", pkgFlex, true, false)

	// --- 4. Network Status ---
	netView := newOutputView()
	refreshNet := func() {
		netView.Clear()
		fmt.Fprint(netView, netStatusReport())
	}

	netStatusBtn := tview.NewButton("Refresh Status").SetSelectedFunc(refreshNet)
	netAddrBtn := tview.NewButton("Show Interfaces (ip addr)").SetSelectedFunc(func() {
		runInto(netView, "ip", "addr")
	})

	netControls := tview.NewFlex().
		AddItem(netStatusBtn, 0, 1, true).
		AddItem(netAddrBtn, 0, 1, false)

	netFlex := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(netControls, 3, 1, true).
		AddItem(netView, 0, 4, false)

	pages.AddPage("Network", netFlex, true, false)

	// --- 5. Wi-Fi Setup ---
	// Saving here is the only way to configure wireless now that boot no
	// longer prompts; vakt-net notices the new file and connects on its own.
	wifiResult := tview.NewTextView().SetDynamicColors(true)
	wifiResult.SetBorder(true).SetTitle(" Result ")

	savedSSID, savedIface := readNetConfig()
	wifiForm := tview.NewForm().
		AddInputField("SSID", savedSSID, 30, nil, nil).
		AddPasswordField("Password", "", 30, '*', nil).
		AddInputField("Interface", savedIface, 12, nil, nil)

	wifiForm.AddButton("Save & Connect", func() {
		ssid := wifiForm.GetFormItem(0).(*tview.InputField).GetText()
		psk := wifiForm.GetFormItem(1).(*tview.InputField).GetText()
		iface := wifiForm.GetFormItem(2).(*tview.InputField).GetText()

		wifiResult.Clear()
		if ssid == "" {
			fmt.Fprint(wifiResult, "[red]SSID cannot be empty.[-]\n")
			return
		}
		if iface == "" {
			iface = "wlan0"
		}

		path, err := writeNetConfig(ssid, psk, iface)
		if err != nil {
			fmt.Fprintf(wifiResult, "[red]Failed to write %s: %v[-]\n", path, err)
			return
		}
		fmt.Fprintf(wifiResult, "[green]Saved to %s[-]\n\n", path)
		fmt.Fprint(wifiResult, "vakt-net will pick this up within a second and\n"+
			"reconnect. Watch progress on the Network page.\n")
	})
	wifiForm.SetBorder(true).SetTitle(" Wi-Fi Configuration ")

	wifiFlex := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(wifiForm, 11, 1, true).
		AddItem(wifiResult, 0, 1, false)

	pages.AddPage("WiFi", wifiFlex, true, false)

	// --- 6. Services ---
	servicesView := newOutputView()
	servicesView.SetTitle(" Services ")
	refreshServices := func() {
		servicesView.Clear()
		fmt.Fprint(servicesView, servicesReport())
	}

	servicesBtn := tview.NewButton("Refresh").SetSelectedFunc(refreshServices)
	servicesLogBtn := tview.NewButton("vakt-net Log").SetSelectedFunc(func() {
		runInto(servicesView, "cat", "/run/vakt-net.log")
	})

	servicesControls := tview.NewFlex().
		AddItem(servicesBtn, 0, 1, true).
		AddItem(servicesLogBtn, 0, 1, false)

	servicesFlex := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(servicesControls, 3, 1, true).
		AddItem(servicesView, 0, 4, false)

	pages.AddPage("Services", servicesFlex, true, false)

	// --- 7. Intrusion Detection ---
	idsView := newOutputView()
	idsView.SetTitle(" Alerts ")
	refreshIDS := func() {
		idsView.Clear()
		fmt.Fprint(idsView, idsReport(50))
	}

	idsBtn := tview.NewButton("Refresh Alerts").SetSelectedFunc(refreshIDS)
	idsFlex := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(idsBtn, 3, 1, true).
		AddItem(idsView, 0, 4, false)

	pages.AddPage("IDS", idsFlex, true, false)

	// --- Menu List ---
	list := tview.NewList().
		AddItem("Dashboard", "Overview of system status", 'd', func() {
			pages.SwitchToPage("Dashboard")
		}).
		AddItem("Security Scanner", "Run vakt-audit", 's', func() {
			pages.SwitchToPage("Audit")
			app.SetFocus(runAuditBtn)
		}).
		AddItem("Package Manager", "Install software with zrpkg", 'p', func() {
			pages.SwitchToPage("Packages")
			app.SetFocus(pkgInput)
		}).
		AddItem("Network Status", "Check link state and addresses", 'n', func() {
			pages.SwitchToPage("Network")
			refreshNet()
			app.SetFocus(netStatusBtn)
		}).
		AddItem("Wi-Fi Setup", "Configure wireless credentials", 'w', func() {
			pages.SwitchToPage("WiFi")
			app.SetFocus(wifiForm)
		}).
		AddItem("Services", "Inspect background daemons", 'v', func() {
			pages.SwitchToPage("Services")
			refreshServices()
			app.SetFocus(servicesBtn)
		}).
		AddItem("Intrusion Detection", "Review vakt-ids alerts", 'i', func() {
			pages.SwitchToPage("IDS")
			refreshIDS()
			app.SetFocus(idsBtn)
		}).
		AddItem("Graphical Mode", "Hand the console to vakt-compositor", 'g', func() {
			launchCompositor(app, dashboardText, pages)
		}).
		AddItem("Exit to Shell", "Drop to raw root prompt", 'q', func() {
			app.Stop()
		})
	list.SetBorder(true).SetTitle(" Main Menu ")

	// Global ESC key to return to menu
	app.SetInputCapture(func(event *tcell.EventKey) *tcell.EventKey {
		if event.Key() == tcell.KeyEsc {
			app.SetFocus(list)
		}
		return event
	})

	// Layout
	flex := tview.NewFlex().
		AddItem(list, 0, 1, true).
		AddItem(pages, 0, 3, false)

	title := tview.NewTextView().
		SetText(" VAKT OS SECURITY APPLIANCE ").
		SetTextAlign(tview.AlignCenter).
		SetTextColor(tcell.ColorRed)

	mainLayout := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(title, 1, 1, false).
		AddItem(flex, 0, 1, true)

	if err := app.SetRoot(mainLayout, true).EnableMouse(true).Run(); err != nil {
		panic(err)
	}
}

// launchCompositor drops out of the TUI and gives the raw console to the
// framebuffer compositor, then restores the panel when it exits. Suspend is
// what makes this safe: tview releases the terminal and stops drawing, so the
// compositor is not fighting it for the console.
func launchCompositor(app *tview.Application, report *tview.TextView, pages *tview.Pages) {
	pages.SwitchToPage("Dashboard")

	var runErr error
	suspended := app.Suspend(func() {
		fmt.Print("\033[2J\033[H")
		fmt.Println("Switching to graphical mode. The compositor renders to /dev/fb0.")

		cmd := exec.Command("vakt-compositor")
		cmd.Stdin = os.Stdin
		cmd.Stdout = os.Stdout
		cmd.Stderr = os.Stderr
		runErr = cmd.Run()

		// Wipe the console before the TUI redraws over the framebuffer.
		fmt.Print("\033[2J\033[H")
	})

	report.Clear()
	switch {
	case !suspended:
		fmt.Fprint(report, "[red]Could not suspend the panel; graphical mode unavailable.[-]\n")
	case runErr != nil:
		fmt.Fprintf(report, "[red]vakt-compositor exited with an error: %v[-]\n\n", runErr)
		fmt.Fprint(report, "This usually means /dev/fb0 is missing - the kernel needs a\n"+
			"framebuffer console for graphical mode to work.\n")
	default:
		fmt.Fprint(report, "[green]Returned from graphical mode.[-]\n")
	}
}
