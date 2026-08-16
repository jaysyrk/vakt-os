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

// The persistent path when the disk is mounted, the RAM-only one otherwise.
func repoConfPath() string {
	if info, err := os.Stat("/persistent"); err == nil && info.IsDir() {
		return persistentRepoConf
	}
	return fallbackRepoConf
}

// The precedence zrpkg itself applies: environment, persistent, image, built-in.
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

// The scheme check mirrors zrpkg's, so a bad URL is rejected while the form is
// still on screen rather than at the next install.
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

	// World-readable: not a secret, and zrpkg may run as root or as the panel.
	body := "# Vakt OS package repository.\n" +
		"# Packages are verified against /etc/vakt/trusted.key whatever server\n" +
		"# they come from, so this setting decides where to fetch, not what to\n" +
		"# trust.\n" +
		"repo_url=" + url + "\n"

	return url, path, durable.WriteFile(path, []byte(body), 0644)
}
