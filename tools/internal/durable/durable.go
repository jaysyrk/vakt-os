// Package durable writes files that survive losing power.
package durable

import (
	"os"
	"path/filepath"
)

// WriteFile replaces path with data, atomically and durably.
//
// os.WriteFile followed by os.Rename is not enough on ext4: the rename is
// committed before the data behind it, so a power cut inside the writeback
// window leaves the file at its final name with zero length. A zero-length
// file is not a harmless partial write - vakt-panel reads an unparseable auth
// file as no PIN at all, which unlocks the console.
//
// So the temporary file is fsynced before the rename, and the directory is
// fsynced after it, which is what makes the rename itself durable.
func WriteFile(path string, data []byte, perm os.FileMode) error {
	tmp := path + ".tmp"

	f, err := os.OpenFile(tmp, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, perm)
	if err != nil {
		return err
	}

	// Not left to the open: perm is masked by the umask, and these files
	// carry PINs and Wi-Fi passwords.
	if err := f.Chmod(perm); err != nil {
		f.Close()
		os.Remove(tmp)
		return err
	}
	if _, err := f.Write(data); err != nil {
		f.Close()
		os.Remove(tmp)
		return err
	}
	if err := f.Sync(); err != nil {
		f.Close()
		os.Remove(tmp)
		return err
	}
	if err := f.Close(); err != nil {
		os.Remove(tmp)
		return err
	}

	if err := os.Rename(tmp, path); err != nil {
		os.Remove(tmp)
		return err
	}

	d, err := os.Open(filepath.Dir(path))
	if err != nil {
		return err
	}
	defer d.Close()
	return d.Sync()
}

// WriteInPlace truncates path and rewrites it, keeping the same inode.
//
// For files a Landlock ruleset already names: a rule is keyed on the inode
// that existed when the ruleset was sealed, so replacing the file by rename
// leaves the daemon pointing at an inode it can no longer reach. There is no
// atomicity here to trade away - a reader can see a half-written file - only
// durability, which is the part WriteFile's callers were missing.
func WriteInPlace(path string, data []byte, perm os.FileMode) error {
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, perm)
	if err != nil {
		return err
	}
	// O_CREATE's mode is masked by the umask, and ignored outright for a file
	// that already exists - neither of which should decide who can read a PSK.
	if err := f.Chmod(perm); err != nil {
		f.Close()
		return err
	}
	if _, err := f.Write(data); err != nil {
		f.Close()
		return err
	}
	if err := f.Sync(); err != nil {
		f.Close()
		return err
	}
	return f.Close()
}
