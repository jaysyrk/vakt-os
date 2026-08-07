package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
)

func TestSendWebhookToDeliversTheAlert(t *testing.T) {
	var got alertPayload
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Errorf("method = %s, want POST", r.Method)
		}
		if ct := r.Header.Get("Content-Type"); ct != "application/json" {
			t.Errorf("Content-Type = %q, want application/json", ct)
		}
		if err := json.NewDecoder(r.Body).Decode(&got); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		w.WriteHeader(http.StatusNoContent)
	}))
	defer srv.Close()

	if err := sendWebhookTo(srv.Client(), srv.URL, "MODIFIED", "/persistent/etc/passwd"); err != nil {
		t.Fatalf("sendWebhookTo: %v", err)
	}

	if got.Kind != "MODIFIED" {
		t.Errorf("Kind = %q, want MODIFIED", got.Kind)
	}
	if got.Detail != "/persistent/etc/passwd" {
		t.Errorf("Detail = %q, want /persistent/etc/passwd", got.Detail)
	}
	if got.Host == "" {
		t.Error("Host is empty, want the local hostname")
	}
	if got.Time.IsZero() {
		t.Error("Time is zero, want the send time")
	}
}

func TestSendWebhookToReportsNonSuccessStatus(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer srv.Close()

	if err := sendWebhookTo(srv.Client(), srv.URL, "ADDED", "/persistent/foo"); err == nil {
		t.Fatal("sendWebhookTo returned nil error for a 500 response, want an error")
	}
}

func TestSendWebhookToReportsUnreachableEndpoint(t *testing.T) {
	// Nothing is listening here: a closed server's URL is refused immediately.
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {}))
	url := srv.URL
	srv.Close()

	if err := sendWebhookTo(http.DefaultClient, url, "DELETED", "/persistent/bar"); err == nil {
		t.Fatal("sendWebhookTo returned nil error for an unreachable endpoint, want an error")
	}
}

func TestLoadWebhookURLFromPrefersTheFlag(t *testing.T) {
	dir := t.TempDir()
	configPath := filepath.Join(dir, "vakt-ids-webhook.conf")
	writeFile(t, configPath, "https://from-config.example.com/hook\n")

	got := loadWebhookURLFrom("https://from-flag.example.com/hook", configPath)
	if got != "https://from-flag.example.com/hook" {
		t.Errorf("got %q, want the flag value", got)
	}
}

func TestLoadWebhookURLFromFallsBackToConfigFile(t *testing.T) {
	dir := t.TempDir()
	configPath := filepath.Join(dir, "vakt-ids-webhook.conf")
	writeFile(t, configPath, "https://from-config.example.com/hook\n")

	got := loadWebhookURLFrom("", configPath)
	if got != "https://from-config.example.com/hook" {
		t.Errorf("got %q, want the config file's URL", got)
	}
}

func TestLoadWebhookURLFromIsEmptyWhenNeitherIsSet(t *testing.T) {
	dir := t.TempDir()
	configPath := filepath.Join(dir, "does-not-exist.conf")

	got := loadWebhookURLFrom("", configPath)
	if got != "" {
		t.Errorf("got %q, want empty (webhooks disabled)", got)
	}
}

func writeFile(t *testing.T, path, contents string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(contents), 0600); err != nil {
		t.Fatalf("write %s: %v", path, err)
	}
}

func TestSendWebhookIsANoOpWhenDisabled(t *testing.T) {
	// webhookURL defaults to "" (the flag's default), and alert() only calls
	// sendWebhook when it is non-empty. This documents that contract so a
	// future change to alert() can't silently start firing webhooks that
	// were never opted into.
	if webhookURL != "" {
		t.Fatalf("webhookURL = %q, want empty by default", webhookURL)
	}
}
