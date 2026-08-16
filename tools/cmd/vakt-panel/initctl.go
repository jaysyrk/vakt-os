package main

import (
	"fmt"
	"net"
	"os"
)

// Root-owned, group-writable by the panel's account: the only reason an
// unprivileged panel can ask for a shutdown at all.
const initSocket = "/run/init.sock"

// verb is "poweroff", "reboot", or "halt". Asking init rather than calling
// reboot(2) means the ordered shutdown runs with the disk still mounted.
func requestShutdown(verb string) error {
	if _, err := os.Stat(initSocket); err != nil {
		return fmt.Errorf("vakt-init is not listening on %s", initSocket)
	}

	conn, err := net.Dial("unixgram", initSocket)
	if err != nil {
		return fmt.Errorf("cannot reach vakt-init: %w", err)
	}
	defer conn.Close()

	if _, err := conn.Write([]byte("SHUTDOWN=" + verb + "\n")); err != nil {
		return fmt.Errorf("cannot send the request: %w", err)
	}
	return nil
}
