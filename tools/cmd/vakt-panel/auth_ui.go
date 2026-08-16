package main

import (
	"fmt"

	"github.com/gdamore/tcell/v2"
	"github.com/rivo/tview"
)

// What the panel shows first. onUnlock must run before the root swap: arming
// the Esc handler earlier targets a primitive the lock screen does not contain,
// leaving the form deaf to the keyboard.
func authGateRoot(app *tview.Application, mainLayout, focusAfter tview.Primitive, onUnlock func()) tview.Primitive {
	unlock := func() {
		onUnlock()
		app.SetRoot(mainLayout, true).SetFocus(focusAfter)
	}

	if hasPIN() {
		return lockScreen(unlock)
	}
	// A damaged auth file reaches setup too (see hasPINAt), so say which it is.
	return setupScreen(unlock, pinDamaged())
}

// tview's Form treats Enter as "move to the next element", so a correct PIN
// entered and submitted looks broken. The capture belongs on the fields.
func submitOnEnter(form *tview.Form, submit func()) {
	for i := 0; i < form.GetFormItemCount(); i++ {
		input, ok := form.GetFormItem(i).(*tview.InputField)
		if !ok {
			continue
		}
		input.SetInputCapture(func(event *tcell.EventKey) *tcell.EventKey {
			if event.Key() == tcell.KeyEnter {
				submit()
				return nil
			}
			return event
		})
	}
}

func gateFrame(inner tview.Primitive, innerHeight int) tview.Primitive {
	banner := tview.NewTextView().
		SetDynamicColors(true).
		SetText(" " + accent + "[::b]VAKT OS" + off + "  " + dim + "security appliance" + off)

	rows := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(banner, 1, 0, false).
		AddItem(rule(), 1, 0, false).
		AddItem(tview.NewBox(), 1, 0, false).
		AddItem(inner, innerHeight, 1, true).
		AddItem(tview.NewBox(), 0, 1, false)

	return tview.NewFlex().
		AddItem(tview.NewBox(), 2, 0, false).
		AddItem(rows, 0, 1, true)
}

func lockScreen(unlock func()) tview.Primitive {
	result := tview.NewTextView().SetDynamicColors(true)
	attempts := 0

	form := tview.NewForm()
	form.AddPasswordField("PIN", "", 20, '*', nil)

	attempt := func() {
		pin := form.GetFormItemByLabel("PIN").(*tview.InputField).GetText()
		if verifyPIN(pin) {
			unlock()
			return
		}
		attempts++
		form.GetFormItemByLabel("PIN").(*tview.InputField).SetText("")
		result.SetText(fmt.Sprintf("[#d75f5f]Incorrect PIN. (%d attempt(s))[-]", attempts))
	}
	form.AddButton("Unlock", attempt)
	submitOnEnter(form, attempt)
	styleForm(form)

	notice := tview.NewTextView().SetDynamicColors(true).
		SetText(section("LOCKED") + "\n" + dim + "Enter the panel PIN to continue." + off)

	layout := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(notice, 3, 0, false).
		AddItem(form, 6, 1, true).
		AddItem(result, 1, 0, false)
	return gateFrame(layout, 10)
}

// Shown once, on first start. A forgotten PIN is recoverable: vakt.rootshell on
// the kernel command line can delete the stored file.
func setupScreen(unlock func(), damaged bool) tview.Primitive {
	result := tview.NewTextView().SetDynamicColors(true)

	message := "[#d7af5f]No PIN is set. Anyone with console access has full control\n" +
		"of this appliance - Wi-Fi credentials, installed packages, and\n" +
		"shutdown. Set one now, or skip and set it later from the\n" +
		"Panel Lock page.[-]"
	if damaged {
		message = "[#d75f5f]The stored PIN could not be read. The file is there but this\n" +
			"panel cannot use it - unreadable, or not owned by this account -\n" +
			"so no PIN would ever have unlocked it. Setting one now replaces\n" +
			"it. Check ownership from a root shell first if you want the old\n" +
			"one back: /persistent/etc/vakt-panel.auth[-]"
	}
	notice := tview.NewTextView().SetDynamicColors(true).SetText(message)

	form := tview.NewForm()
	form.AddPasswordField("New PIN", "", 20, '*', nil)
	form.AddPasswordField("Confirm PIN", "", 20, '*', nil)
	save := func() {
		pin := form.GetFormItemByLabel("New PIN").(*tview.InputField).GetText()
		confirm := form.GetFormItemByLabel("Confirm PIN").(*tview.InputField).GetText()

		if pin == "" {
			result.SetText("[#d75f5f]PIN cannot be empty.[-]")
			return
		}
		if pin != confirm {
			result.SetText("[#d75f5f]PIN and confirmation do not match.[-]")
			return
		}
		if err := setPIN(pin); err != nil {
			result.SetText(fmt.Sprintf("[#d75f5f]Could not save PIN: %v[-]", err))
			return
		}
		unlock()
	}
	form.AddButton("Set PIN", save)
	form.AddButton("Skip (not recommended)", unlock)
	// Enter saves; Skip stays a deliberate Tab-and-press.
	submitOnEnter(form, save)
	styleForm(form)

	layout := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(notice, 4, 0, false).
		AddItem(form, 8, 1, true).
		AddItem(result, 1, 0, false)
	return gateFrame(layout, 13)
}
