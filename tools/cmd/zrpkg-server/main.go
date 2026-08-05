package main

import (
	"flag"
	"log"
	"net/http"
	"os"
	"path/filepath"
)

func main() {
	dir := flag.String("dir", "./repo", "Directory containing .zrp packages and index.json")
	addr := flag.String("addr", ":8080", "Address to listen on")
	flag.Parse()

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

	// Log every request so you can watch the VM pull packages in real time.
	fs := http.FileServer(http.Dir(repoDir))
	http.Handle("/", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		log.Printf("%s %s %s", r.RemoteAddr, r.Method, r.URL.Path)
		fs.ServeHTTP(w, r)
	}))

	log.Printf("ZRPKG Server listening on %s", *addr)
	log.Printf("Serving packages from: %s", repoDir)
	log.Printf("Under QEMU user networking the VM reaches this host at http://10.0.2.2%s", *addr)
	if err := http.ListenAndServe(*addr, nil); err != nil {
		log.Fatal(err)
	}
}
