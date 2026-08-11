package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"vakt-os/tools/internal/durable"
)

const (
	persistentRepoConf = "/persistent/etc/zrpkg.conf"
	fallbackRepoConf   = "/etc/vakt/zrpkg.conf"
	defaultRepoURL     = "http://10.0.2.2:8080"
)

// repoConfPath returns the persistent location when the data disk is mounted,
// so a repository set here survives a reboot, and the RAM-only path otherwise.
func repoConfPath() string {
	if info, err := os.Stat("/persistent"); err == nil && info.IsDir() {
		return persistentRepoConf
	}
	return fallbackRepoConf
}

// readRepoURL loads the configured repository, matching the precedence zrpkg
// itself applies: the environment, then the persistent file, then the image
// default, then the built-in.
func readRepoURL() string {
	if url := strings.TrimSpace(os.Getenv("ZRPKG_REPO_URL")); url != "" {
		return url
	}
	for _, path := range []string{persistentRepoConf, fallbackRepoConf} {
		if url := repoURLFrom(path); url != "" {
			return url
		}
	}
	return defaultRepoURL
}

func repoURLFrom(path string) string {
	data, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	for _, line := range strings.Split(string(data), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		key, value, found := strings.Cut(line, "=")
		if !found {
			continue
		}
		switch strings.ToLower(strings.TrimSpace(key)) {
		case "repo_url", "repo", "url":
			if value = strings.TrimSpace(value); value != "" {
				return value
			}
		}
	}
	return ""
}

// writeRepoURL points this system at a different repository.
//
// The scheme check mirrors zrpkg's: anything but http or https would be a way
// to make the package manager read from somewhere nobody intended. Validating
// here as well means the panel rejects it while the user is still looking at
// the form, rather than at the next install.
func writeRepoURL(raw string) (string, string, error) {
	url := strings.TrimRight(strings.TrimSpace(raw), "/")
	if url == "" {
		return "", "", fmt.Errorf("enter a URL like https://packages.example.com")
	}

	scheme, rest, found := strings.Cut(url, "://")
	if !found || rest == "" {
		return "", "", fmt.Errorf("enter a URL like https://packages.example.com")
	}
	switch strings.ToLower(scheme) {
	case "http", "https":
	default:
		return "", "", fmt.Errorf("%q is not a scheme zrpkg can fetch from; use http or https", scheme)
	}

	path := repoConfPath()
	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return url, path, err
	}

	// Deliberately world-readable: unlike the Wi-Fi configuration next to it,
	// a repository URL is not a secret, and zrpkg may run as either root or the
	// panel's own account.
	body := "# Vakt OS package repository.\n" +
		"# Packages are verified against /etc/vakt/trusted.key whatever server\n" +
		"# they come from, so this setting decides where to fetch, not what to\n" +
		"# trust.\n" +
		"repo_url=" + url + "\n"

	return url, path, durable.WriteFile(path, []byte(body), 0644)
}
