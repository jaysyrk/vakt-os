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
// The panel cannot do this itself. Signalling PID 1 needs privileges it
// deliberately does not have, and calling reboot(2) from here would cut power
// to a mounted disk with daemons still writing to it. Sending the request to
// init instead means the ordered sequence - stop services, sync, unmount, power
// off - happens whichever way the shutdown was asked for.
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
