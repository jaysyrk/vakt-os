package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestRepoURLFromParsesTheConfigFile(t *testing.T) {
	path := writeTemp(t, "zrpkg.conf",
		"# the VPS\n\nport=9000\nrepo_url = https://packages.example.com \n")

	if got := repoURLFrom(path); got != "https://packages.example.com" {
		t.Errorf("got %q", got)
	}
}

func TestRepoURLFromAcceptsShorterKeys(t *testing.T) {
	for _, key := range []string{"repo_url", "repo", "url", "URL"} {
		path := writeTemp(t, "zrpkg.conf", key+"=https://a.example\n")
		if got := repoURLFrom(path); got != "https://a.example" {
			t.Errorf("key %q: got %q", key, got)
		}
	}
}

func TestRepoURLFromHandlesMissingOrEmptyFiles(t *testing.T) {
	if got := repoURLFrom(filepath.Join(t.TempDir(), "absent")); got != "" {
		t.Errorf("missing file should yield nothing, got %q", got)
	}
	path := writeTemp(t, "zrpkg.conf", "# nothing set\nrepo_url=\n")
	if got := repoURLFrom(path); got != "" {
		t.Errorf("empty value should yield nothing, got %q", got)
	}
}

// The panel validates before writing so a bad URL is rejected while the user is
// still looking at the form, rather than at the next install.
func TestWriteRepoURLRejectsUnusableSchemes(t *testing.T) {
	for _, bad := range []string{
		"", "   ", "packages.example.com", "file:///etc/passwd",
		"ftp://packages.example.com", "https://",
	} {
		if _, _, err := writeRepoURL(bad); err == nil {
			t.Errorf("%q should have been rejected", bad)
		}
	}
}

func TestWriteRepoURLNormalisesAndRoundTrips(t *testing.T) {
	// Redirect the write at a scratch directory rather than the real /etc.
	dir := t.TempDir()
	t.Setenv("HOME", dir)

	url, _, err := writeRepoURL("  https://packages.example.com/  ")
	if err != nil && !os.IsPermission(err) {
		t.Fatalf("unexpected error: %v", err)
	}
	if url != "https://packages.example.com" {
		t.Errorf("trailing slash and whitespace should be stripped, got %q", url)
	}

	// Whatever the destination, the rendered body has to be something
	// repoURLFrom can read back - that is the contract with zrpkg.
	body := filepath.Join(dir, "zrpkg.conf")
	rendered := "# comment\nrepo_url=" + url + "\n"
	if err := os.WriteFile(body, []byte(rendered), 0644); err != nil {
		t.Fatal(err)
	}
	if got := repoURLFrom(body); got != url {
		t.Errorf("round trip failed: got %q, want %q", got, url)
	}
}

func TestReadRepoURLPrefersTheEnvironment(t *testing.T) {
	t.Setenv("ZRPKG_REPO_URL", "https://from-the-environment.example")
	if got := readRepoURL(); got != "https://from-the-environment.example" {
		t.Errorf("got %q", got)
	}
}

func TestReadRepoURLFallsBackToTheQemuHost(t *testing.T) {
	t.Setenv("ZRPKG_REPO_URL", "")
	got := readRepoURL()
	// On a developer machine neither config file exists, so this is the
	// built-in default; on a machine that has one, it is whatever that says.
	if got == "" {
		t.Error("readRepoURL should never return an empty string")
	}
	if !strings.Contains(got, "://") {
		t.Errorf("got something that is not a URL: %q", got)
	}
}
