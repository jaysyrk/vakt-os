package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gdamore/tcell/v2"
	"github.com/rivo/tview"
)

func TestSetAndVerifyPIN(t *testing.T) {
	path := filepath.Join(t.TempDir(), "vakt-panel.auth")

	if hasPINAt(path) {
		t.Fatal("a PIN should not exist before one is set")
	}
	if err := setPINAt(path, "1234"); err != nil {
		t.Fatalf("setPINAt: %v", err)
	}
	if !hasPINAt(path) {
		t.Fatal("a PIN should exist after being set")
	}
	if !verifyPINAt(path, "1234") {
		t.Error("the correct PIN should verify")
	}
	if verifyPINAt(path, "4321") {
		t.Error("an incorrect PIN must not verify")
	}
}

func TestSetPINOverwritesThePrevious(t *testing.T) {
	path := filepath.Join(t.TempDir(), "vakt-panel.auth")
	if err := setPINAt(path, "1111"); err != nil {
		t.Fatalf("setPINAt: %v", err)
	}
	if err := setPINAt(path, "2222"); err != nil {
		t.Fatalf("setPINAt: %v", err)
	}

	if verifyPINAt(path, "1111") {
		t.Error("the old PIN should no longer verify")
	}
	if !verifyPINAt(path, "2222") {
		t.Error("the new PIN should verify")
	}
}

// Two identical PINs must not produce identical stored files, or the file
// itself would leak whether two appliances (or two generations of PIN on the
// same one) share a PIN.
func TestEachStoredPINUsesADifferentSalt(t *testing.T) {
	a := filepath.Join(t.TempDir(), "a")
	b := filepath.Join(t.TempDir(), "b")
	if err := setPINAt(a, "1234"); err != nil {
		t.Fatalf("setPINAt: %v", err)
	}
	if err := setPINAt(b, "1234"); err != nil {
		t.Fatalf("setPINAt: %v", err)
	}

	da, _ := os.ReadFile(a)
	db, _ := os.ReadFile(b)
	if string(da) == string(db) {
		t.Error("identical PINs produced identical stored hashes")
	}
}

func TestVerifyPINFailsGracefullyWithNoFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "absent")
	if verifyPINAt(path, "anything") {
		t.Error("no stored PIN should never verify")
	}
}

func TestRemovePIN(t *testing.T) {
	path := filepath.Join(t.TempDir(), "vakt-panel.auth")
	if err := setPINAt(path, "1234"); err != nil {
		t.Fatalf("setPINAt: %v", err)
	}
	if err := removePINAt(path); err != nil {
		t.Fatalf("removePINAt: %v", err)
	}
	if hasPINAt(path) {
		t.Error("PIN should be gone after removal")
	}
	// Removing an already-absent PIN is not a failure: the caller's intent
	// ("no PIN configured") already holds.
	if err := removePINAt(path); err != nil {
		t.Errorf("removing an already-absent PIN should be harmless: %v", err)
	}
}

func TestSetPINLeavesNoTempFileBehind(t *testing.T) {
	path := filepath.Join(t.TempDir(), "vakt-panel.auth")
	if err := setPINAt(path, "1234"); err != nil {
		t.Fatalf("setPINAt: %v", err)
	}
	if _, err := os.Stat(path + ".tmp"); !os.IsNotExist(err) {
		t.Errorf("a temp file was left behind: %v", err)
	}
}

// A file that exists but cannot be parsed must not present a lock screen:
// verifyPINAt fails safe for every candidate against it, the correct PIN
// included, so treating it as "a PIN is set" locks the console permanently.
func TestAnUnusableAuthFileIsNotTreatedAsAConfiguredPIN(t *testing.T) {
	valid := filepath.Join(t.TempDir(), "good")
	if err := setPINAt(valid, "1234"); err != nil {
		t.Fatalf("setPINAt: %v", err)
	}
	if got := storedPIN(valid); got != pinUsable {
		t.Errorf("a freshly written PIN should be usable, got %v", got)
	}
	if !hasPINAt(valid) {
		t.Error("a freshly written PIN should count as configured")
	}

	for name, body := range map[string]string{
		"empty":            "",
		"no separator":     "deadbeef",
		"salt not hex":     "zzzz:" + strings.Repeat("ab", 32),
		"salt wrong size":  "abcd:" + strings.Repeat("ab", 32),
		"digest not hex":   strings.Repeat("ab", 16) + ":zzzz",
		"digest truncated": strings.Repeat("ab", 16) + ":" + strings.Repeat("ab", 8),
		// The realistic one: a write cut short by power loss.
		"truncated mid-write": strings.Repeat("ab", 16) + ":" + strings.Repeat("cd", 20),
	} {
		path := filepath.Join(t.TempDir(), "auth")
		if err := os.WriteFile(path, []byte(body), 0600); err != nil {
			t.Fatal(err)
		}
		if got := storedPIN(path); got != pinUnusable {
			t.Errorf("%s: expected pinUnusable, got %v", name, got)
		}
		if hasPINAt(path) {
			t.Errorf("%s: an unusable file must not count as a configured PIN", name)
		}
		if verifyPINAt(path, "1234") {
			t.Errorf("%s: nothing may verify against an unusable file", name)
		}
	}
}

func TestAbsentAuthFileIsAbsentNotDamaged(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nothing-here")
	if got := storedPIN(path); got != pinAbsent {
		t.Errorf("a missing file should be pinAbsent, got %v", got)
	}
}

func TestMalformedStoredPINDoesNotVerify(t *testing.T) {
	path := filepath.Join(t.TempDir(), "vakt-panel.auth")
	if err := os.WriteFile(path, []byte("not-the-expected-format"), 0600); err != nil {
		t.Fatal(err)
	}
	if verifyPINAt(path, "anything") {
		t.Error("a malformed auth file must not verify")
	}
}

// Enter inside a PIN field must submit. tview's Form treats Enter as "move to
// the next element", which silently swallows the key everyone presses after
// typing a PIN - the lock screen then does nothing at all, and a correct PIN
// is indistinguishable from a wrong one.
func TestEnterInAPINFieldSubmits(t *testing.T) {
	form := tview.NewForm()
	form.AddPasswordField("PIN", "", 20, '*', nil)

	submitted := 0
	form.AddButton("Unlock", func() { submitted++ })
	submitOnEnter(form, func() { submitted++ })

	field := form.GetFormItem(0)
	press := func(key tcell.Key) {
		field.InputHandler()(tcell.NewEventKey(key, 0, tcell.ModNone), func(tview.Primitive) {})
	}

	press(tcell.KeyEnter)
	if submitted != 1 {
		t.Fatalf("Enter in the PIN field should have submitted once, got %d", submitted)
	}

	// Tab must still mean "move on" rather than submit, or there is no way to
	// reach a second button (Skip, on the setup screen) without triggering the
	// first one.
	press(tcell.KeyTab)
	if submitted != 1 {
		t.Errorf("Tab must not submit; submit count went to %d", submitted)
	}
}

// The button and the Enter key have to run the same code, or one of them
// drifts and only some users hit the working path.
func TestEnterAndTheButtonRunTheSameAction(t *testing.T) {
	form := tview.NewForm()
	form.AddPasswordField("PIN", "", 20, '*', nil)

	var ran []string
	action := func() { ran = append(ran, "action") }
	form.AddButton("Unlock", action)
	submitOnEnter(form, action)

	form.GetFormItem(0).InputHandler()(
		tcell.NewEventKey(tcell.KeyEnter, 0, tcell.ModNone), func(tview.Primitive) {})
	form.GetButton(0).InputHandler()(
		tcell.NewEventKey(tcell.KeyEnter, 0, tcell.ModNone), func(tview.Primitive) {})

	if len(ran) != 2 {
		t.Errorf("Enter and the button should both run the action, got %d run(s)", len(ran))
	}
}

// An auth file the panel cannot read is not an appliance without a PIN.
//
// This is the failure that actually shipped: the file was written while the
// panel was running as root, stayed 0600 root-owned, and every later boot ran
// the panel as an unprivileged user that got EACCES. Reporting that as
// pinAbsent showed an ordinary first-boot setup screen and rejected the
// correct PIN with nothing on screen to explain why.
func TestAnUnreadableAuthFileIsNotReportedAsNoPIN(t *testing.T) {
	check := func(what, path string) {
		t.Helper()
		if got := storedPIN(path); got != pinUnusable {
			t.Errorf("%s: should be pinUnusable, got %v", what, got)
		}
		if !pinDamagedAt(path) {
			t.Errorf("%s: the setup screen must say the stored PIN could not be read", what)
		}
		if hasPINAt(path) {
			t.Errorf("%s: must not serve a lock screen nothing can open", what)
		}
	}

	// EISDIR rather than EACCES, so this still exercises the branch when the
	// suite runs as root - which is exactly where a permissions fixture would
	// quietly skip and stop guarding anything.
	dir := filepath.Join(t.TempDir(), "vakt-panel.auth")
	if err := os.Mkdir(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	check("a path that is not a regular file", dir)

	// The real shape: a 0600 file owned by someone else. Root bypasses file
	// permissions, so this half only means anything unprivileged.
	if os.Geteuid() != 0 {
		denied := filepath.Join(t.TempDir(), "vakt-panel.auth")
		if err := setPINAt(denied, "1234"); err != nil {
			t.Fatalf("setPINAt: %v", err)
		}
		if err := os.Chmod(denied, 0o000); err != nil {
			t.Fatal(err)
		}
		check("an unreadable file", denied)
	}
}
