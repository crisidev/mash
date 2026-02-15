use std::os::unix::io::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;

use color_eyre::eyre::{self, Context};
use nix::pty::openpty;
use nix::sys::termios;
use nix::unistd::ForkResult;

nix::ioctl_write_int_bad!(tiocsctty, nix::libc::TIOCSCTTY);

pub(crate) struct PtyChild {
    pub(crate) master_fd: OwnedFd,
    pub(crate) pid: i32,
}

pub(crate) fn spawn_ssh(hostname: &str, port: &str, ssh_template: &str, user: Option<&str>) -> eyre::Result<PtyChild> {
    let pty_result = openpty(None, None).wrap_err("openpty failed")?;

    match unsafe { nix::unistd::fork().wrap_err("fork failed")? } {
        ForkResult::Child => {
            // Child process - drop master
            drop(pty_result.master);

            nix::unistd::setsid().ok();
            unsafe { tiocsctty(pty_result.slave.as_raw_fd(), 0) }.ok();
            nix::unistd::dup2_stdin(&pty_result.slave).ok();
            nix::unistd::dup2_stdout(&pty_result.slave).ok();
            nix::unistd::dup2_stderr(&pty_result.slave).ok();
            if pty_result.slave.as_raw_fd() > 2 {
                drop(pty_result.slave);
            } else {
                // Don't close if it's 0, 1, or 2 since we just dup2'd to those
                std::mem::forget(pty_result.slave);
            }

            let name = match user {
                Some(u) => format!("{}@{}", u, hostname),
                None => hostname.to_string(),
            };
            let port_arg = if port != "22" {
                format!("-p {}", port)
            } else {
                String::new()
            };

            let mut evaluated = ssh_template.replace("%(host)s", &name).replace("%(port)s", &port_arg);

            // If template didn't contain %(host)s, append the host
            if evaluated == ssh_template.replace("%(port)s", &port_arg) && !evaluated.contains(&name) {
                evaluated = format!("{} {}", evaluated, name);
            }

            let err = std::process::Command::new("/bin/sh").arg("-c").arg(&evaluated).exec();

            eprintln!("exec failed: {}", err);
            std::process::exit(1);
        }
        ForkResult::Parent { child } => {
            // Drop slave in parent
            drop(pty_result.slave);

            // Configure master PTY: disable echo, disable ONLCR
            if let Ok(mut attrs) = termios::tcgetattr(pty_result.master.as_fd()) {
                attrs.output_flags.remove(termios::OutputFlags::ONLCR);
                attrs.local_flags.remove(termios::LocalFlags::ECHO);
                let _ = termios::tcsetattr(pty_result.master.as_fd(), termios::SetArg::TCSANOW, &attrs);
            }

            Ok(PtyChild {
                master_fd: pty_result.master,
                pid: child.as_raw(),
            })
        }
    }
}
