package main

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

const (
	persistentAuthFile = "/persistent/etc/vakt-panel.auth"
	fallbackAuthFile   = "/etc/vakt/panel.auth"
	saltBytes          = 16
)

// authFilePath returns the persistent location when the data disk is
// mounted, so a configured PIN survives a reboot, and the RAM-only path
// otherwise - the same precedence vaktnet.go and zrpkgconf.go use for their
// own configuration.
func authFilePath() string {
	if info, err := os.Stat("/persistent"); err == nil && info.IsDir() {
		return persistentAuthFile
	}
	return fallbackAuthFile
}

func hasPIN() bool              { return hasPINAt(authFilePath()) }
func setPIN(pin string) error   { return setPINAt(authFilePath(), pin) }
func verifyPIN(pin string) bool { return verifyPINAt(authFilePath(), pin) }
func removePIN() error          { return removePINAt(authFilePath()) }
func pinDamaged() bool          { return pinDamagedAt(authFilePath()) }

func pinDamagedAt(path string) bool { return storedPIN(path) == pinUnusable }

// What is actually sitting at the auth file path.
type pinState int

const (
	pinAbsent pinState = iota
	pinUsable
	// The file is there but nothing can be checked against it.
	pinUnusable
)

// storedPIN reports whether the auth file holds something verifiable.
// verifyPINAt fails safe for every candidate against a malformed value, the
// correct PIN included, so "exists" is not the same as "has a PIN".
func storedPIN(path string) pinState {
	data, err := os.ReadFile(path)
	if err != nil {
		// Only a missing file means no PIN. Anything else - typically a 0600
		// file owned by root rather than the panel's user - means the PIN
		// cannot be checked, which is not the same thing.
		if os.IsNotExist(err) {
			return pinAbsent
		}
		return pinUnusable
	}

	saltHex, wantHex, found := strings.Cut(strings.TrimSpace(string(data)), ":")
	if !found {
		return pinUnusable
	}
	salt, err := hex.DecodeString(saltHex)
	if err != nil || len(salt) != saltBytes {
		return pinUnusable
	}
	digest, err := hex.DecodeString(wantHex)
	if err != nil || len(digest) != sha256.Size {
		return pinUnusable
	}
	return pinUsable
}

// hasPINAt reports whether a usable PIN is configured.
//
// An unusable file counts as no PIN rather than a locked console: damaging it
// requires already running as the panel's user, and `vakt.rootshell` is the
// documented way past a forgotten PIN anyway. A console nobody can ever open
// again is the worse outcome. setupScreen says so in red.
func hasPINAt(path string) bool {
	return storedPIN(path) == pinUsable
}

// setPINAt hashes and stores a new PIN, replacing any existing one. Each call
// draws a fresh salt, so two appliances (or two PINs set on the same one over
// time) never produce the same stored bytes for the same PIN.
func setPINAt(path, pin string) error {
	salt := make([]byte, saltBytes)
	if _, err := rand.Read(salt); err != nil {
		return fmt.Errorf("could not generate a salt: %w", err)
	}

	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return err
	}

	body := hex.EncodeToString(salt) + ":" + hashPIN(pin, salt) + "\n"
	// Temp-then-rename: a write cut short by power loss would otherwise leave
	// a half-written file, which storedPIN reads as a damaged PIN.
	tmp := path + ".tmp"
	// Root-only: this file exists to deny the console to whoever does not
	// know the PIN, so it must not be readable by the account it is
	// protecting the panel from.
	if err := os.WriteFile(tmp, []byte(body), 0600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// verifyPINAt checks a candidate PIN against the stored hash in constant
// time, so a wrong guess cannot be distinguished from a right one by how long
// the comparison took.
func verifyPINAt(path, pin string) bool {
	data, err := os.ReadFile(path)
	if err != nil {
		return false
	}
	saltHex, wantHex, found := strings.Cut(strings.TrimSpace(string(data)), ":")
	if !found {
		return false
	}
	salt, err := hex.DecodeString(saltHex)
	if err != nil {
		return false
	}
	got := hashPIN(pin, salt)
	return subtle.ConstantTimeCompare([]byte(got), []byte(wantHex)) == 1
}

// removePINAt deletes the stored PIN. Removing one that is already gone is
// not an error - the caller's intent ("no PIN should exist") is satisfied
// either way.
func removePINAt(path string) error {
	if err := os.Remove(path); err != nil && !os.IsNotExist(err) {
		return err
	}
	return nil
}

func hashPIN(pin string, salt []byte) string {
	h := sha256.New()
	h.Write(salt)
	h.Write([]byte(pin))
	return hex.EncodeToString(h.Sum(nil))
}
