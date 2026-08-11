// zrpkg-server publishes a signed Vakt OS package repository over HTTP.
//
// The surface is deliberately small: read-only methods, a flat namespace, two
// file extensions, no directory listings, bounded timeouts, per-IP rate
// limiting, and optional TLS for when nothing is proxying in front of it.
//
// No authentication. Packages are signed and clients verify them against a
// trust anchor, so the repository is public data - keep the signing key off
// this machine and there is nothing here worth stealing.
package main

import (
	"context"
	"errors"
	"flag"
	"log"
	"net/http"
	"os"
	"os/signal"
	"path"
	"path/filepath"
	"strings"
	"syscall"
	"time"
)

// The only extensions a repository consists of: <name>.zrp archives and
// <name>.json manifests, of which index.json is one. Anything else in the
// directory - a stray key, an editor backup, a note to self - is not served.
var servedExtensions = map[string]bool{
	".zrp":  true,
	".json": true,
}

const (
	readTimeout     = 15 * time.Second
	writeTimeout    = 5 * time.Minute // a .zrp over a slow link
	idleTimeout     = 60 * time.Second
	headerTimeout   = 10 * time.Second
	shutdownTimeout = 10 * time.Second
)

func main() {
	dir := flag.String("dir", "./repo", "Directory containing .zrp packages and index.json")
	addr := flag.String("addr", ":8080", "Address to listen on")
	tlsCert := flag.String("tls-cert", "", "PEM certificate chain; enables HTTPS when set with -tls-key")
	tlsKey := flag.String("tls-key", "", "PEM private key for -tls-cert")
	quiet := flag.Bool("quiet", false, "Do not log individual requests")
	rateLimit := flag.Float64("rate-limit", 5, "Max sustained requests per second, per client IP (0 disables)")
	rateBurst := flag.Float64("rate-burst", 20, "Requests a client IP may burst before the rate limit applies")
	trustProxy := flag.Bool("trust-proxy", false, "Trust X-Real-IP from a direct loopback connection for rate "+
		"limiting - only set this when running behind a reverse proxy on the same host that sets it, or every "+
		"client collapses into one shared rate limit")
	flag.Parse()

	log.SetFlags(log.LstdFlags | log.LUTC)

	repoDir, err := filepath.Abs(*dir)
	if err != nil {
		log.Fatalf("Invalid repo directory: %v", err)
	}
	if err := os.MkdirAll(repoDir, 0755); err != nil {
		log.Fatalf("Failed to create repo directory: %v", err)
	}
	if _, err := os.Stat(filepath.Join(repoDir, "index.json")); err != nil {
		log.Printf("Warning: no index.json in %s - run build-system/mkrepo.sh first", repoDir)
	}

	if (*tlsCert == "") != (*tlsKey == "") {
		log.Fatal("-tls-cert and -tls-key must be given together")
	}
	serveTLS := *tlsCert != ""

	handler := repoHandler(repoDir, *quiet)
	if *rateLimit > 0 {
		handler = newRateLimiter(*rateLimit, *rateBurst, *trustProxy).limit(handler)
	}

	server := &http.Server{
		Addr:              *addr,
		Handler:           handler,
		ReadTimeout:       readTimeout,
		ReadHeaderTimeout: headerTimeout,
		WriteTimeout:      writeTimeout,
		IdleTimeout:       idleTimeout,
		ErrorLog:          log.Default(),
	}

	scheme := "http"
	if serveTLS {
		scheme = "https"
	}
	log.Printf("ZRPKG Server listening on %s (%s)", *addr, scheme)
	log.Printf("Serving packages from: %s", repoDir)
	log.Printf("Point an appliance at it with: zrpkg repo %s://<this-host>", scheme)
	if !serveTLS {
		log.Print("No TLS: signatures still protect what clients install, but " +
			"anyone on the path can see what they install. Terminate TLS here " +
			"with -tls-cert/-tls-key, or in a reverse proxy.")
	}
	if *rateLimit > 0 {
		log.Printf("Rate limit: %.0f req/s per IP, burst %.0f", *rateLimit, *rateBurst)
		if *trustProxy {
			log.Print("Rate limit: trusting X-Real-IP from loopback connections")
		} else {
			log.Print("Rate limit: keyed on the direct connection's address - if a reverse " +
				"proxy sits in front of this, every client shares one bucket unless -trust-proxy is set")
		}
	} else {
		log.Print("Rate limit: disabled (-rate-limit 0)")
	}

	// A rented server gets restarted, redeployed and reconfigured. Draining in
	// flight downloads on the way out means a package fetch that was halfway
	// through finishes rather than becoming a truncated file the client then
	// reports as a signature failure.
	shutdown := make(chan os.Signal, 1)
	signal.Notify(shutdown, syscall.SIGINT, syscall.SIGTERM)

	go func() {
		<-shutdown
		log.Print("Shutting down; waiting for in-flight requests...")
		ctx, cancel := context.WithTimeout(context.Background(), shutdownTimeout)
		defer cancel()
		if err := server.Shutdown(ctx); err != nil {
			log.Printf("Shutdown: %v", err)
		}
	}()

	if serveTLS {
		err = server.ListenAndServeTLS(*tlsCert, *tlsKey)
	} else {
		err = server.ListenAndServe()
	}
	if err != nil && !errors.Is(err, http.ErrServerClosed) {
		log.Fatal(err)
	}
	log.Print("Stopped.")
}

// repoHandler serves the repository directory and nothing else.
func repoHandler(repoDir string, quiet bool) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !quiet {
			log.Printf("%s %s %s", r.RemoteAddr, r.Method, r.URL.Path)
		}

		// A repository is read-only. Anything else is either a mistake or a
		// probe, and both deserve the same answer.
		if r.Method != http.MethodGet && r.Method != http.MethodHead {
			w.Header().Set("Allow", "GET, HEAD")
			http.Error(w, "read-only repository", http.StatusMethodNotAllowed)
			return
		}

		name, ok := repoFile(r.URL.Path)
		if !ok {
			http.NotFound(w, r)
			return
		}

		full := filepath.Join(repoDir, name)
		info, err := os.Stat(full)
		if err != nil || !info.Mode().IsRegular() {
			http.NotFound(w, r)
			return
		}

		// Manifests and the index change every time the repository is rebuilt;
		// an archive at a given name is immutable in practice but not
		// guaranteed, so nothing is cached for long.
		w.Header().Set("Cache-Control", "no-cache")
		http.ServeFile(w, r, full)
	})
}

// repoFile maps a request path to a filename inside the repository, or reports
// that there is nothing legitimate to serve.
//
// The repository is flat: every valid request is for a single file at the root.
// Collapsing to a bare filename and rejecting anything with a directory
// component means traversal is not something to be defended against so much as
// something with nowhere to go - "/../../etc/passwd" does not resolve to a
// filename this function will return.
func repoFile(urlPath string) (string, bool) {
	// path.Clean resolves "." and ".." within the URL, and a leading slash
	// makes any attempt to climb past the root resolve back to it.
	cleaned := path.Clean("/" + urlPath)
	name := strings.TrimPrefix(cleaned, "/")

	if name == "" || strings.Contains(name, "/") {
		return "", false
	}
	if !servedExtensions[strings.ToLower(path.Ext(name))] {
		return "", false
	}
	return name, true
}
