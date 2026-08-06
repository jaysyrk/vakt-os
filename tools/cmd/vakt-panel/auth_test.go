package main

import (
	"os"
	"path/filepath"
	"testing"
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

func TestMalformedStoredPINDoesNotVerify(t *testing.T) {
	path := filepath.Join(t.TempDir(), "vakt-panel.auth")
	if err := os.WriteFile(path, []byte("not-the-expected-format"), 0600); err != nil {
		t.Fatal(err)
	}
	if verifyPINAt(path, "anything") {
		t.Error("a malformed auth file must not verify")
	}
}
