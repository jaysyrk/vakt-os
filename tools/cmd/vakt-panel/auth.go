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

func hasPINAt(path string) bool {
	_, err := os.Stat(path)
	return err == nil
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
	// Written under a temp name and renamed into place, so a write
	// interrupted by power loss can never leave a corrupt-but-existing file
	// at path: hasPINAt only checks the file exists, and a corrupt file
	// there permanently locks the console out - verifyPINAt fails safe for
	// every candidate PIN, correct or not.
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
