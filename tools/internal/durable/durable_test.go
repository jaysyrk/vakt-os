package durable

import (
	"os"
	"path/filepath"
	"syscall"
	"testing"
)

func inodeOf(t *testing.T, path string) uint64 {
	t.Helper()
	info, err := os.Stat(path)
	if err != nil {
		t.Fatalf("stat %s: %v", path, err)
	}
	sys, ok := info.Sys().(*syscall.Stat_t)
	if !ok {
		t.Skip("no inode information on this platform")
	}
	return sys.Ino
}

func TestWriteFileReplacesContentAndLeavesNoTemp(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "conf")

	if err := WriteFile(path, []byte("first"), 0600); err != nil {
		t.Fatalf("first write: %v", err)
	}
	if err := WriteFile(path, []byte("second"), 0600); err != nil {
		t.Fatalf("second write: %v", err)
	}

	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read back: %v", err)
	}
	if string(got) != "second" {
		t.Errorf("content = %q, want %q", got, "second")
	}

	if _, err := os.Stat(path + ".tmp"); !os.IsNotExist(err) {
		t.Error("the temporary file outlived the write")
	}
}

// The auth file holds a PIN and the net config holds a PSK, so the mode has to
// come out right whatever umask the panel inherited.
func TestModeIsNotLeftToTheUmask(t *testing.T) {
	old := syscall.Umask(0077)
	defer syscall.Umask(old)

	dir := t.TempDir()
	for name, write := range map[string]func(string, []byte, os.FileMode) error{
		"WriteFile":    WriteFile,
		"WriteInPlace": WriteInPlace,
	} {
		path := filepath.Join(dir, name)
		if err := write(path, []byte("x"), 0644); err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		info, err := os.Stat(path)
		if err != nil {
			t.Fatalf("%s stat: %v", name, err)
		}
		if got := info.Mode().Perm(); got != 0644 {
			t.Errorf("%s mode = %04o, want 0644", name, got)
		}
	}
}

// vakt-net's Landlock rule is keyed on the inode that existed when its ruleset
// was sealed. Replacing the config by rename would leave the daemon pointing at
// an inode it can no longer reach, so this file must be rewritten in place.
func TestWriteInPlaceKeepsTheSameInode(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "vakt-net.conf")

	if err := os.WriteFile(path, []byte("ssid=old\n"), 0600); err != nil {
		t.Fatal(err)
	}
	before := inodeOf(t, path)

	if err := WriteInPlace(path, []byte("ssid=new\n"), 0600); err != nil {
		t.Fatalf("write in place: %v", err)
	}

	if after := inodeOf(t, path); after != before {
		t.Errorf("inode changed (%d -> %d); a Landlock rule for this path would "+
			"no longer reach it", before, after)
	}

	got, _ := os.ReadFile(path)
	if string(got) != "ssid=new\n" {
		t.Errorf("content = %q", got)
	}
}

// WriteFile, by contrast, is free to allocate a new inode - and does, which is
// why it must not be used for the paths Landlock names.
func TestWriteFileIsAllowedToReplaceTheInode(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "zrpkg.conf")

	if err := WriteFile(path, []byte("a"), 0644); err != nil {
		t.Fatal(err)
	}
	before := inodeOf(t, path)
	if err := WriteFile(path, []byte("b"), 0644); err != nil {
		t.Fatal(err)
	}
	if inodeOf(t, path) == before {
		t.Skip("same inode reused by chance; nothing to assert")
	}
}

func TestWriteFileReportsAnUnwritableDirectory(t *testing.T) {
	if err := WriteFile(filepath.Join(t.TempDir(), "no", "such", "dir", "f"),
		[]byte("x"), 0600); err == nil {
		t.Error("expected an error for a missing parent directory")
	}
}
