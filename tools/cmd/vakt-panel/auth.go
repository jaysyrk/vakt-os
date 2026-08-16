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
	"vakt-os/tools/internal/durable"
)

const (
	persistentAuthFile = "/persistent/etc/vakt-panel.auth"
	fallbackAuthFile   = "/etc/vakt/panel.auth"
	saltBytes          = 16
)

// Same precedence as vaktnet.go and zrpkgconf.go.
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

// "Exists" is not "has a PIN": verifyPINAt fails safe on a malformed value.
func storedPIN(path string) pinState {
	data, err := os.ReadFile(path)
	if err != nil {
		// Anything but a missing file means the PIN cannot be checked.
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

// An unusable file counts as no PIN rather than a permanently locked console;
// setupScreen says so in red.
func hasPINAt(path string) bool {
	return storedPIN(path) == pinUsable
}

// Each call draws a fresh salt.
func setPINAt(path, pin string) error {
	salt := make([]byte, saltBytes)
	if _, err := rand.Read(salt); err != nil {
		return fmt.Errorf("could not generate a salt: %w", err)
	}

	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return err
	}

	body := hex.EncodeToString(salt) + ":" + hashPIN(pin, salt) + "\n"
	// Durable, not just atomic: an unsynced rename can leave a zero-length file
	// after a power cut, which reads as no PIN at all - an unlocked console.
	return durable.WriteFile(path, []byte(body), 0600)
}

// Constant time: a wrong guess must not be distinguishable by duration.
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

// Removing a PIN that is already gone is not an error.
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
