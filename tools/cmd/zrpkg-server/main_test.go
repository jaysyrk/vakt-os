package main

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestRepoFileAcceptsRepositoryFiles(t *testing.T) {
	for _, urlPath := range []string{
		"/index.json",
		"/vakt-audit.zrp",
		"/vakt-audit.json",
		"/VAKT-AUDIT.ZRP",
	} {
		name, ok := repoFile(urlPath)
		if !ok {
			t.Errorf("%q should be servable", urlPath)
			continue
		}
		if strings.Contains(name, "/") {
			t.Errorf("%q produced a path with a directory component: %q", urlPath, name)
		}
	}
}

// The repository is flat and holds two kinds of file. Everything else - the
// signing key someone left in the directory, a listing request, a probe for a
// well-known path - gets nothing.
func TestRepoFileRejectsEverythingElse(t *testing.T) {
	for _, urlPath := range []string{
		"/",
		"",
		"/repo.key",
		"/notes.txt",
		"/subdir/vakt-audit.zrp",
		"/.env",
		"/index.json.bak",
	} {
		if name, ok := repoFile(urlPath); ok {
			t.Errorf("%q should not be servable, got %q", urlPath, name)
		}
	}
}

// Traversal has nowhere to go: a request that climbs out of the root either
// resolves back into it or keeps a directory component, and both are refused.
func TestRepoFileRefusesTraversal(t *testing.T) {
	for _, urlPath := range []string{
		"/../../etc/passwd",
		"/../repo.key",
		"/./../../index.json",
		"//etc/passwd",
		"/a/../../../index.json",
	} {
		name, ok := repoFile(urlPath)
		if ok && name != "index.json" {
			t.Errorf("%q escaped the repository as %q", urlPath, name)
		}
		if ok && strings.Contains(name, "..") {
			t.Errorf("%q produced a traversing name: %q", urlPath, name)
		}
	}
}

func serveTest(t *testing.T) (http.Handler, string) {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "index.json"), []byte(`{"packages":[]}`), 0644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "vakt-audit.zrp"), []byte("archive"), 0644); err != nil {
		t.Fatal(err)
	}
	// A private key someone left in the repository directory by mistake. The
	// server must not hand it out.
	if err := os.WriteFile(filepath.Join(dir, "repo.key"), []byte("secret"), 0600); err != nil {
		t.Fatal(err)
	}
	return repoHandler(dir, true), dir
}

func TestHandlerServesPackages(t *testing.T) {
	handler, _ := serveTest(t)

	for _, path := range []string{"/index.json", "/vakt-audit.zrp"} {
		recorder := httptest.NewRecorder()
		handler.ServeHTTP(recorder, httptest.NewRequest(http.MethodGet, path, nil))
		if recorder.Code != http.StatusOK {
			t.Errorf("GET %s: got %d, want 200", path, recorder.Code)
		}
	}
}

func TestHandlerHidesNonRepositoryFiles(t *testing.T) {
	handler, _ := serveTest(t)

	recorder := httptest.NewRecorder()
	handler.ServeHTTP(recorder, httptest.NewRequest(http.MethodGet, "/repo.key", nil))
	if recorder.Code != http.StatusNotFound {
		t.Errorf("GET /repo.key: got %d, want 404", recorder.Code)
	}
	if strings.Contains(recorder.Body.String(), "secret") {
		t.Error("the signing key was served")
	}
}

// Directory listings enumerate the repository for anyone who asks, which is
// exactly what a public server should not volunteer.
func TestHandlerDoesNotListTheDirectory(t *testing.T) {
	handler, _ := serveTest(t)

	recorder := httptest.NewRecorder()
	handler.ServeHTTP(recorder, httptest.NewRequest(http.MethodGet, "/", nil))
	if recorder.Code != http.StatusNotFound {
		t.Errorf("GET /: got %d, want 404", recorder.Code)
	}
	if strings.Contains(recorder.Body.String(), "vakt-audit") {
		t.Error("the directory contents were listed")
	}
}

func TestHandlerIsReadOnly(t *testing.T) {
	handler, _ := serveTest(t)

	for _, method := range []string{http.MethodPut, http.MethodPost, http.MethodDelete} {
		recorder := httptest.NewRecorder()
		handler.ServeHTTP(recorder, httptest.NewRequest(method, "/vakt-audit.zrp", nil))
		if recorder.Code != http.StatusMethodNotAllowed {
			t.Errorf("%s: got %d, want 405", method, recorder.Code)
		}
	}
}

func TestHandlerAllowsHeadForSizeChecks(t *testing.T) {
	handler, _ := serveTest(t)

	recorder := httptest.NewRecorder()
	handler.ServeHTTP(recorder, httptest.NewRequest(http.MethodHead, "/vakt-audit.zrp", nil))
	if recorder.Code != http.StatusOK {
		t.Errorf("HEAD: got %d, want 200", recorder.Code)
	}
}
