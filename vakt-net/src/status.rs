use std::fmt;
use std::io::Write;
use std::path::Path;

/// Where vakt-panel reads the current link state from.
pub const STATUS_PATH: &str = "/run/vakt-net.status";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No config file yet - the user has not entered Wi-Fi details.
    Unconfigured,
    Connecting,
    Connected,
    Failed,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            State::Unconfigured => "unconfigured",
            State::Connecting => "connecting",
            State::Connected => "connected",
            State::Failed => "failed",
        };
        f.write_str(s)
    }
}

pub struct Status {
    pub state: State,
    pub interface: String,
    pub ssid: Option<String>,
    pub ip: Option<String>,
    pub detail: String,
}

/// Writes the status file atomically so the TUI never reads a half-written one.
pub fn write(status: &Status) {
    let mut body = String::new();
    body.push_str(&format!("state={}\n", status.state));
    body.push_str(&format!("interface={}\n", status.interface));
    body.push_str(&format!("ssid={}\n", status.ssid.as_deref().unwrap_or("")));
    body.push_str(&format!("ip={}\n", status.ip.as_deref().unwrap_or("")));
    body.push_str(&format!("detail={}\n", status.detail));

    let final_path = Path::new(STATUS_PATH);
    let Some(dir) = final_path.parent() else {
        return;
    };
    let _ = std::fs::create_dir_all(dir);

    let tmp_path = dir.join("vakt-net.status.tmp");
    if let Ok(mut file) = std::fs::File::create(&tmp_path)
        && file.write_all(body.as_bytes()).is_ok()
        && file.sync_all().is_ok()
    {
        let _ = std::fs::rename(&tmp_path, final_path);
        return;
    }
    let _ = std::fs::remove_file(&tmp_path);
}
