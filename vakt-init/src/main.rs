mod services;

use nix::mount::{mount, MsFlags};
use std::env;
use std::fs;
use std::process::Command;

const ZRPKG_ROOT: &str = "/persistent/zrpkg";

fn main() {
    unsafe {
        env::set_var("PATH", "/bin:/sbin:/usr/bin:/usr/sbin");
        env::set_var("HOME", "/root");
    }
    let _ = fs::create_dir_all("/root");
    let _ = fs::create_dir_all("/etc/vakt");

    println!("[Vakt-Init] Mounting virtual filesystems...");
    let flags = MsFlags::empty();
    let no_data: Option<&str> = None;

    let _ = mount(Some("proc"), "/proc", Some("proc"), flags, no_data);
    let _ = mount(Some("sys"), "/sys", Some("sysfs"), flags, no_data);
    let _ = mount(Some("dev"), "/dev", Some("devtmpfs"), flags, no_data);

    println!("[Vakt-Init] Isolating volatile RAM filesystems and enforcing Read-Only Root...");
    let secure_tmpfs_flags = MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC;

    let _ = fs::create_dir_all("/run");
    let _ = mount(Some("tmpfs"), "/run", Some("tmpfs"), secure_tmpfs_flags, Some("mode=0755,size=64M"));

    let _ = fs::create_dir_all("/tmp");
    let _ = mount(Some("tmpfs"), "/tmp", Some("tmpfs"), secure_tmpfs_flags, Some("mode=1777,size=64M"));

    let _ = mount(None::<&str>, "/", None::<&str>, MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY, no_data);

    println!("[Vakt-Init] Searching for persistent storage on /dev/sda...");
    let _ = fs::create_dir_all("/persistent");
    let mut persistent = false;
    if let Ok(status) = Command::new("mount")
        .args(&["-t", "ext4", "/dev/sda", "/persistent"])
        .status()
    {
        if status.success() {
            println!("[Vakt-Init] \x1b[1;32mMounted persistent storage successfully!\x1b[0m");
            persistent = true;
        } else {
            println!("[Vakt-Init] \x1b[1;33mNo persistent disk found. Running in RAM only mode.\x1b[0m");
        }
    }

    if persistent {
        let _ = fs::create_dir_all("/persistent/etc");
        let _ = fs::create_dir_all(ZRPKG_ROOT);
        unsafe {
            env::set_var("ZRPKG_ROOT", ZRPKG_ROOT);
        }
        let path = env::var("PATH").unwrap_or_default();
        unsafe {
            env::set_var("PATH", format!("{}/usr/bin:{}", ZRPKG_ROOT, path));
        }
    }

    println!("[Vakt-Init] Loading hardware drivers...");
    let _ = Command::new("sh")
        .arg("-c")
        .arg("find /sys -name modalias -exec cat {} + | xargs -n 1 modprobe 2>/dev/null")
        .status();

    println!("\x1b[1;31m");
    if let Ok(logo) = fs::read_to_string("/etc/vakt_logo.txt") {
        println!("{}", logo);
    }
    println!("\x1b[0m");
    println!("Welcome to Vakt OS (Rust Init System)");
    println!("");

    println!("[Vakt-Init] Starting system services...");
    std::thread::spawn(|| {
        services::Supervisor::new(services::DEFAULT_SERVICES).run();
    });

    unsafe {
        env::set_var("PS1", "\\[\\e[1;31m\\][Vakt-OS]\\[\\e[0m\\] \\w # ");
    }

    loop {
        println!("[Vakt-Init] Launching Vakt Panel...");
        let _ = Command::new("setsid")
            .arg("cttyhack")
            .arg("vakt-panel")
            .status();

        println!("[Vakt-Init] Panel exited. Dropping to raw root shell...");
        let _ = Command::new("setsid")
            .arg("cttyhack")
            .arg("/bin/sh")
            .status();
    }
}
