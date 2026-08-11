package main

import (
	"fmt"
	"net"
	"os"
)

// initSocket is where vakt-init listens for readiness notifications and control
// requests. It is owned by root and group-writable by the account the panel
// runs as, which is the only reason an unprivileged panel can ask for a
// shutdown at all.
const initSocket = "/run/init.sock"

// requestShutdown asks PID 1 to bring the system down.
//
// Signalling PID 1 needs privileges the panel deliberately lacks, and
// reboot(2) here would cut power to a mounted disk with daemons still writing.
// Asking init means the ordered sequence happens however shutdown was invoked.
//
// verb is "poweroff", "reboot", or "halt".
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
