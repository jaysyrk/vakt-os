// vakt-ids is the Vakt OS host intrusion detection daemon.
//
// It takes a SHA-256 baseline of the directories it watches and re-scans them
// on an interval, reporting files that were added, modified, deleted, or had
// their permissions changed. Alerts go to stdout (captured by vakt-init into
// /run/vakt-ids.log) and to an alert file the panel can display.
package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

const (
	alertFile = "/run/vakt-ids.alerts"
	// Files larger than this are tracked by size and mtime only, so a big
	// package archive never stalls a scan.
	maxHashBytes = 32 << 20
	// The alert file is trimmed to this many lines when it grows past it.
	maxAlertLines = 1000
)

// Entry is the recorded state of one file.
type Entry struct {
	Hash  string `json:"hash"`
	Size  int64  `json:"size"`
	Mode  uint32 `json:"mode"`
	MTime int64  `json:"mtime"`
}

type baseline struct {
	Created time.Time        `json:"created"`
	Roots   []string         `json:"roots"`
	Files   map[string]Entry `json:"files"`
}

func main() {
	watch := flag.String("watch", "/persistent", "Comma-separated directories to monitor")
	interval := flag.Duration("interval", 30*time.Second, "Time between integrity scans")
	baselinePath := flag.String("baseline", "", "Where to persist the baseline (default: inside the first watched dir)")
	once := flag.Bool("once", false, "Run a single scan and exit instead of running as a daemon")
	flag.Parse()

	log.SetFlags(0)
	log.SetPrefix("[vakt-ids] ")

	roots := splitRoots(*watch)
	if len(roots) == 0 {
		log.Fatal("No directories to watch")
	}

	fmt.Println("Vakt OS Intrusion Detection System (vakt-ids)")
	fmt.Printf("Watching: %s\n", strings.Join(roots, ", "))

	// In RAM-only mode /persistent may not exist. Wait for it rather than
	// exiting, so the supervisor does not burn through its restart budget.
	available := waitForRoots(roots, *once)
	if len(available) == 0 {
		log.Print("No watched directory exists; nothing to monitor.")
		// Still a settled answer: boot must not wait for a monitor that has
		// decided there is nothing to monitor.
		notifyReady("no watched directory exists")
		return
	}

	statePath := *baselinePath
	if statePath == "" {
		statePath = filepath.Join(available[0], "var", "vakt-ids", "baseline.json")
	}

	base, err := loadBaseline(statePath)
	if err != nil {
		log.Printf("No usable baseline (%v); taking a fresh one.", err)
		base = nil
	}

	if base == nil {
		files, err := scan(available, statePath)
		if err != nil {
			log.Fatalf("Initial scan failed: %v", err)
		}
		base = &baseline{Created: time.Now(), Roots: available, Files: files}
		if err := saveBaseline(statePath, base); err != nil {
			log.Printf("Warning: could not persist baseline: %v", err)
		}
		alert("INFO", fmt.Sprintf("Baseline established: %d files under %s",
			len(files), strings.Join(available, ", ")))
	} else {
		log.Printf("Loaded baseline from %s (%d files, taken %s).",
			statePath, len(base.Files), base.Created.Format(time.RFC3339))
	}

	// The baseline is what makes this daemon useful; before it exists there is
	// nothing to compare against, so this is the first moment it is genuinely
	// up rather than merely running.
	notifyReady(readyStatus(len(base.Files), available))

	if *once {
		runScan(base, available, statePath)
		return
	}

	log.Printf("Monitoring every %s.", *interval)
	for {
		time.Sleep(*interval)
		runScan(base, available, statePath)
	}
}

// runScan compares the current filesystem against the baseline and folds any
// findings back into it, so each change is reported once rather than forever.
func runScan(base *baseline, roots []string, statePath string) {
	current, err := scan(roots, statePath)
	if err != nil {
		log.Printf("Scan failed: %v", err)
		return
	}

	findings := diff(base.Files, current)
	if len(findings) == 0 {
		return
	}

	for _, f := range findings {
		alert(f.Kind, f.Path)
	}
	log.Printf("%d integrity violation(s) detected.", len(findings))

	base.Files = current
	if err := saveBaseline(statePath, base); err != nil {
		log.Printf("Warning: could not update baseline: %v", err)
	}
}

type finding struct {
	Kind string
	Path string
}

// diff reports how current differs from old, in a stable order.
func diff(old, current map[string]Entry) []finding {
	var findings []finding

	for path, now := range current {
		before, existed := old[path]
		switch {
		case !existed:
			findings = append(findings, finding{"ADDED", path})
		case before.Hash != now.Hash || before.Size != now.Size:
			findings = append(findings, finding{"MODIFIED", path})
		case before.Mode != now.Mode:
			findings = append(findings, finding{"PERMISSIONS", fmt.Sprintf(
				"%s (%04o -> %04o)",
				path,
				fs.FileMode(before.Mode).Perm(),
				fs.FileMode(now.Mode).Perm())})
		}
	}

	for path := range old {
		if _, stillThere := current[path]; !stillThere {
			findings = append(findings, finding{"DELETED", path})
		}
	}

	sort.Slice(findings, func(i, j int) bool {
		if findings[i].Kind != findings[j].Kind {
			return findings[i].Kind < findings[j].Kind
		}
		return findings[i].Path < findings[j].Path
	})
	return findings
}

// scan hashes every regular file under roots, skipping the baseline itself so
// writing state never looks like tampering.
func scan(roots []string, statePath string) (map[string]Entry, error) {
	files := make(map[string]Entry)
	skip, _ := filepath.Abs(statePath)

	for _, root := range roots {
		err := filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
			if err != nil {
				// An unreadable subtree should not abort the whole scan.
				return nil
			}
			if d.IsDir() {
				return nil
			}
			// Symlinks and devices have no meaningful content hash.
			if !d.Type().IsRegular() {
				return nil
			}
			if abs, e := filepath.Abs(path); e == nil && abs == skip {
				return nil
			}

			info, err := d.Info()
			if err != nil {
				return nil
			}

			entry := Entry{
				Size:  info.Size(),
				Mode:  uint32(info.Mode()),
				MTime: info.ModTime().Unix(),
			}
			if info.Size() <= maxHashBytes {
				h, err := hashFile(path)
				if err != nil {
					return nil
				}
				entry.Hash = h
			} else {
				entry.Hash = fmt.Sprintf("size:%d", info.Size())
			}

			files[path] = entry
			return nil
		})
		if err != nil {
			return nil, err
		}
	}
	return files, nil
}

func hashFile(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()

	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return "", err
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}

func loadBaseline(path string) (*baseline, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var b baseline
	if err := json.Unmarshal(data, &b); err != nil {
		return nil, err
	}
	if b.Files == nil {
		return nil, fmt.Errorf("baseline contains no files")
	}
	return &b, nil
}

// saveBaseline writes atomically so an interrupted write cannot corrupt state.
func saveBaseline(path string, b *baseline) error {
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		return err
	}
	data, err := json.Marshal(b)
	if err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// alert records a finding to stdout and to the panel-readable alert file.
func alert(kind, detail string) {
	line := fmt.Sprintf("%s\t%s\t%s", time.Now().Format(time.RFC3339), kind, detail)
	fmt.Println(line)

	f, err := os.OpenFile(alertFile, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0600)
	if err != nil {
		return
	}
	fmt.Fprintln(f, line)
	f.Close()

	trimAlerts()
}

func trimAlerts() {
	data, err := os.ReadFile(alertFile)
	if err != nil {
		return
	}
	lines := strings.Split(strings.TrimRight(string(data), "\n"), "\n")
	if len(lines) <= maxAlertLines {
		return
	}
	kept := strings.Join(lines[len(lines)-maxAlertLines:], "\n") + "\n"
	_ = os.WriteFile(alertFile, []byte(kept), 0600)
}

func splitRoots(raw string) []string {
	var roots []string
	for _, part := range strings.Split(raw, ",") {
		if part = strings.TrimSpace(part); part != "" {
			roots = append(roots, part)
		}
	}
	return roots
}

// waitForRoots blocks until at least one watched directory exists, reporting
// once so the log explains the wait. In --once mode it does not block.
func waitForRoots(roots []string, once bool) []string {
	announced := false
	for {
		var available []string
		for _, root := range roots {
			if info, err := os.Stat(root); err == nil && info.IsDir() {
				available = append(available, root)
			}
		}
		if len(available) > 0 || once {
			return available
		}
		if !announced {
			log.Printf("Waiting for %s to become available...", strings.Join(roots, ", "))
			announced = true
		}
		time.Sleep(15 * time.Second)
	}
}
