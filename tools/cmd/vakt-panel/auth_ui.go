package main

import (
	"fmt"

	"github.com/gdamore/tcell/v2"
	"github.com/rivo/tview"
)

// authGateRoot returns the primitive vakt-panel should show first: a PIN
// prompt when one is configured, a one-time setup screen when it is not.
// Either path ends by calling unlock, which swaps the running application's
// root for the real panel - tview allows changing the root at any time, so
// there is no second call to app.Run().
//
// onUnlock runs before the swap. main.go uses it to arm the global Esc
// handler, which targets the main menu - a primitive that does not exist
// anywhere in the lock/setup screen's tree, so firing it early leaves no
// focused primitive in the visible screen at all and the form stops taking
// keyboard input for the rest of the session.
func authGateRoot(app *tview.Application, mainLayout, focusAfter tview.Primitive, onUnlock func()) tview.Primitive {
	unlock := func() {
		onUnlock()
		app.SetRoot(mainLayout, true).SetFocus(focusAfter)
	}

	if hasPIN() {
		return lockScreen(unlock)
	}
	// A damaged auth file reaches setup too (see hasPINAt), so say which
	// situation this is. Silently showing first-boot setup on an appliance
	// that had a PIN yesterday would be its own kind of alarming.
	return setupScreen(unlock, pinDamaged())
}

// submitOnEnter makes Enter inside any of the form's input fields run submit.
//
// tview's Form treats Enter on a field as "move to the next element", so a
// PIN form built the obvious way swallows the Enter every person presses
// after typing: focus shifts to the button and nothing else happens - no
// attempt, no error message, no counter. The correct PIN looks broken, and on
// a lock screen that is the difference between getting in and not.
//
// The capture goes on the fields rather than the form because the focused
// primitive is the field itself, and that is what the application hands the
// event to - a capture on the form would never run.
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
		SetTextAlign(tview.AlignCenter).
		SetTextColor(tcell.ColorRed).
		SetText(" VAKT OS SECURITY APPLIANCE ")

	return tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(banner, 1, 0, false).
		AddItem(inner, innerHeight, 1, true).
		AddItem(tview.NewBox(), 0, 1, false)
}

// lockScreen asks for the PIN already on file. It never leaves without a
// correct answer - there is no "skip" here, unlike setup.
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
		result.SetText(fmt.Sprintf("[red]Incorrect PIN. (%d attempt(s))[-]", attempts))
	}
	form.AddButton("Unlock", attempt)
	submitOnEnter(form, attempt)
	form.SetBorder(true).SetTitle(" Locked ")

	layout := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(form, 6, 1, true).
		AddItem(result, 1, 0, false)
	return gateFrame(layout, 7)
}

// setupScreen runs once, the first time the panel ever starts: it offers to
// set a PIN and explains what skipping it means, rather than silently
// leaving the console open. A root recovery shell (vakt.rootshell on the
// kernel command line) can always delete the stored PIN file if one set here
// is later forgotten.
func setupScreen(unlock func(), damaged bool) tview.Primitive {
	result := tview.NewTextView().SetDynamicColors(true)

	message := "[yellow]No PIN is set. Anyone with console access has full control\n" +
		"of this appliance - Wi-Fi credentials, installed packages, and\n" +
		"shutdown. Set one now, or skip and set it later from the\n" +
		"Panel Lock page.[-]"
	if damaged {
		message = "[red]The stored PIN could not be read. The file is there but this\n" +
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
			result.SetText("[red]PIN cannot be empty.[-]")
			return
		}
		if pin != confirm {
			result.SetText("[red]PIN and confirmation do not match.[-]")
			return
		}
		if err := setPIN(pin); err != nil {
			result.SetText(fmt.Sprintf("[red]Could not save PIN: %v[-]", err))
			return
		}
		unlock()
	}
	form.AddButton("Set PIN", save)
	form.AddButton("Skip (not recommended)", unlock)
	// Enter saves; Skip stays a deliberate Tab-and-press, which is the right
	// way round for a step whose whole point is not being breezed past.
	submitOnEnter(form, save)
	form.SetBorder(true).SetTitle(" Protect This Panel ")

	layout := tview.NewFlex().SetDirection(tview.FlexRow).
		AddItem(notice, 4, 0, false).
		AddItem(form, 8, 1, true).
		AddItem(result, 1, 0, false)
	return gateFrame(layout, 13)
}
