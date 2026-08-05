package main

import (
	"fmt"
	"net"
	"os"
	"strings"
)

// notifySocketEnv is set by vakt-init for every service it supervises. When it
// is absent the daemon is being run by hand and there is nobody to tell.
const notifySocketEnv = "NOTIFY_SOCKET"

// notifyReady tells vakt-init the daemon has finished starting, so boot stops
// waiting on it and draws the panel.
//
// The protocol is newline-separated KEY=value pairs in a single Unix datagram,
// the same shape as sd_notify. Nothing here can fail in a way worth acting on:
// a daemon that cannot reach PID 1 still has a filesystem to watch, and
// refusing to work over a missed notification would turn a cosmetic problem
// into an outage.
func notifyReady(status string) {
	send("READY=1\nNAME=vakt-ids\nSTATUS=" + oneLine(status) + "\n")
}

func send(message string) {
	path := os.Getenv(notifySocketEnv)
	if path == "" {
		return
	}

	conn, err := net.Dial("unixgram", path)
	if err != nil {
		return
	}
	defer conn.Close()
	_, _ = conn.Write([]byte(message))
}

// oneLine flattens status text: a newline in a value would be read as the start
// of the next key.
func oneLine(text string) string {
	return strings.Join(strings.Fields(text), " ")
}

// readyStatus describes the baseline in the one line the panel has room for.
func readyStatus(fileCount int, roots []string) string {
	return fmt.Sprintf("watching %d file(s) under %s", fileCount, strings.Join(roots, ", "))
}
