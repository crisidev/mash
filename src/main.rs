mod callbacks;
mod cli;
mod completion;
mod console;
mod control_commands;
mod display_names;
mod host_syntax;
mod input;
mod pty_spawn;
mod shell;
mod shell_manager;
mod signals;

use std::io::IsTerminal;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use color_eyre::eyre::{self, Context, bail};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use owo_colors::OwoColorize;
use tokio::io::AsyncReadExt;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;
use tokio::time::Instant;

use cli::parse_args;
use console::Console;
use display_names::DisplayNameRegistry;
use host_syntax::expand_syntax;
use input::{InputEvent, InputRequest};
use shell::{ShellId, ShellState};
use shell_manager::ShellManager;
use signals::SignalEvent;

enum ShellEvent {
    Data { id: ShellId, data: Vec<u8> },
    Closed { id: ShellId, exit_code: i32 },
}

async fn pty_reader_task(id: ShellId, master_fd: OwnedFd, pid: i32, event_tx: mpsc::Sender<ShellEvent>) {
    // Set non-blocking
    let flags = nix::fcntl::fcntl(master_fd.as_fd(), nix::fcntl::FcntlArg::F_GETFL).unwrap_or(0);
    let mut oflags = nix::fcntl::OFlag::from_bits_truncate(flags);
    oflags.insert(nix::fcntl::OFlag::O_NONBLOCK);
    let _ = nix::fcntl::fcntl(master_fd.as_fd(), nix::fcntl::FcntlArg::F_SETFL(oflags));

    let raw_fd = master_fd.as_raw_fd();
    // Forget the OwnedFd so it doesn't close when dropped — we manage lifetime via AsyncFd
    std::mem::forget(master_fd);

    let async_fd = match AsyncFd::new(raw_fd) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = event_tx.send(ShellEvent::Closed { id, exit_code: 255 }).await;
            return;
        }
    };

    let mut buf = [0u8; 4096];
    loop {
        let mut ready = match async_fd.readable().await {
            Ok(r) => r,
            Err(_) => break,
        };

        match ready.try_io(|inner| {
            let fd = inner.as_raw_fd();
            let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
            match nix::unistd::read(borrowed, &mut buf) {
                Ok(n) => Ok(n),
                Err(nix::errno::Errno::EAGAIN) => Err(std::io::Error::from(std::io::ErrorKind::WouldBlock)),
                Err(e) => Err(std::io::Error::other(e)),
            }
        }) {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => {
                let mut data = buf[..n].to_vec();
                for b in data.iter_mut() {
                    if *b == b'\r' {
                        *b = b'\n';
                    }
                }
                if event_tx.send(ShellEvent::Data { id, data }).await.is_err() {
                    break;
                }
            }
            Ok(Err(e)) => {
                if e.kind() != std::io::ErrorKind::WouldBlock {
                    break;
                }
            }
            Err(_) => continue,
        }
    }

    // Prevent AsyncFd from closing the fd (we manage it ourselves)
    std::mem::forget(async_fd);

    let exit_code = match nix::sys::wait::waitpid(Pid::from_raw(pid), None) {
        Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => code,
        Ok(nix::sys::wait::WaitStatus::Signaled(_, sig, _)) => 128 + sig as i32,
        _ => 255,
    };

    let _ = event_tx.send(ShellEvent::Closed { id, exit_code }).await;
}

fn kill_all(mgr: &ShellManager) {
    for shell in mgr.all_shells() {
        let _ = signal::kill(Pid::from_raw(-shell.pid), Signal::SIGKILL);
    }
}

fn spawn_shell(
    host_str: &str,
    args: &cli::Args,
    command: &Option<String>,
    password: &Option<String>,
    mgr: &mut ShellManager,
    display_names: &mut DisplayNameRegistry,
    shell_event_tx: &mpsc::Sender<ShellEvent>,
) -> eyre::Result<()> {
    let (hostname, port) = host_syntax::split_port(host_str);
    let child = pty_spawn::spawn_ssh(&hostname, &port, &args.ssh, args.user.as_deref())
        .wrap_err_with(|| format!("Failed to spawn ssh to {}", host_str))?;

    let master_fd_for_reader = child.master_fd.try_clone().wrap_err("Failed to clone master fd")?;

    let id = mgr.add_shell(
        hostname,
        port,
        child.pid,
        child.master_fd,
        args.debug,
        command.clone(),
        password.clone(),
        display_names,
    );
    let tx = shell_event_tx.clone();
    tokio::spawn(pty_reader_task(id, master_fd_for_reader, child.pid, tx));
    Ok(())
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    unsafe {
        signal::signal(Signal::SIGPIPE, signal::SigHandler::SigDfl).ok();
    }

    let args = parse_args();

    let interactive = args.command.is_none() && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();

    let command = if !std::io::stdin().is_terminal() && args.command.is_none() {
        let mut stdin_data = String::new();
        tokio::io::stdin()
            .read_to_string(&mut stdin_data)
            .await
            .wrap_err("Failed to read from stdin")?;
        if !stdin_data.is_empty() && !stdin_data.ends_with('\n') {
            stdin_data.push('\n');
        }
        if stdin_data.is_empty() { None } else { Some(stdin_data) }
    } else {
        args.command.clone()
    };

    // Expand hosts
    let mut hosts: Vec<String> = Vec::new();
    for host in &args.host_names {
        hosts.extend(expand_syntax(host));
    }

    if hosts.is_empty() {
        bail!("No hosts given");
    }

    // Read password if needed
    let password = if args.password_file.as_deref() == Some("-") {
        Some(rpassword::prompt_password("Password: ").wrap_err("Failed to read password")?)
    } else if let Some(ref path) = args.password_file {
        let content = tokio::fs::read_to_string(path)
            .await
            .wrap_err_with(|| format!("Failed to read password file: {}", path))?;
        Some(content.lines().next().unwrap_or("").to_string())
    } else {
        None
    };

    // Raise RLIMIT_NOFILE
    let needed = 3 + hosts.len() * 3;
    let (soft, hard) = nix::sys::resource::getrlimit(nix::sys::resource::Resource::RLIMIT_NOFILE)
        .wrap_err("Failed to get RLIMIT_NOFILE")?;
    if needed as u64 > soft {
        let new_hard = std::cmp::max(needed as u64, hard);
        nix::sys::resource::setrlimit(nix::sys::resource::Resource::RLIMIT_NOFILE, needed as u64, new_hard)
            .wrap_err_with(|| {
                format!(
                    "Failed to change RLIMIT_NOFILE from soft={} hard={} to soft={} hard={}",
                    soft, hard, needed, new_hard
                )
            })?;
    }

    // Save terminal state for restoration on exit
    let saved_termios = if interactive {
        nix::sys::termios::tcgetattr(std::io::stdin().as_fd()).ok()
    } else {
        None
    };

    let use_color = !args.no_color && std::io::stdout().is_terminal();
    let mut display_names = DisplayNameRegistry::new();
    let mut mgr = ShellManager::new(use_color);
    let mut console = Console::new(interactive, args.log_file.clone()).await;
    let mut exit_code: i32 = 0;

    let (shell_event_tx, mut shell_event_rx) = mpsc::channel::<ShellEvent>(256);
    let (signal_tx, mut signal_rx) = mpsc::channel::<SignalEvent>(16);

    tokio::spawn(signals::signal_listener(signal_tx));

    // Spawn SSH processes
    for (i, host_str) in hosts.iter().enumerate() {
        if interactive {
            eprint!("Started {}/{} remote processes\r", i, hosts.len());
        }
        if let Err(e) = spawn_shell(
            host_str,
            &args,
            &command,
            &password,
            &mut mgr,
            &mut display_names,
            &shell_event_tx,
        ) {
            eprintln!("{:#}", e);
            if args.abort_errors {
                bail!("Aborting due to --abort-errors");
            }
        }
    }
    if interactive && !hosts.is_empty() {
        eprint!("{}\r", " ".repeat(40));
    }

    // Keep a clone for dynamic :add/:reconnect
    let persistent_shell_tx = shell_event_tx.clone();
    drop(shell_event_tx);

    // Input setup
    let completion_state = Arc::new(RwLock::new(completion::CompletionState::from_manager(&mgr)));
    let (input_req_tx, mut input_resp_rx) = if interactive {
        let (req_tx, resp_rx) = input::spawn_input_thread(completion_state.clone());
        (Some(req_tx), Some(resp_rx))
    } else {
        (None, None)
    };

    let mut input_requested = false;
    let mut next_signal: Option<SignalEvent> = None;
    let mut drain_deadline: Option<Instant> = None;
    const DRAIN_TIMEOUT: Duration = Duration::from_millis(200);

    loop {
        // Handle pending signal
        if let Some(sig) = next_signal.take() {
            match sig {
                SignalEvent::Int => {
                    if interactive {
                        console.log(b"> ^C\n").await;
                        for shell in mgr.all_shells_mut() {
                            if shell.enabled {
                                shell.write_to_pty(b"\x03");
                            }
                        }
                        console.output(b"").await;
                    } else {
                        kill_all(&mgr);
                        std::process::exit(128 + Signal::SIGINT as i32);
                    }
                }
                SignalEvent::Tstp => {
                    console.log(b"> ^Z\n").await;
                    for shell in mgr.all_shells_mut() {
                        if shell.enabled {
                            shell.write_to_pty(b"\x1a");
                        }
                    }
                    console.output(b"").await;
                }
                SignalEvent::Winch => {
                    let (cols, rows) = terminal_size::terminal_size()
                        .map(|(w, h)| (w.0, h.0))
                        .unwrap_or((80, 25));
                    let adjusted_cols = std::cmp::max(
                        cols as i32 - display_names.max_display_name_length as i32 - 2,
                        std::cmp::min(cols as i32, 10),
                    ) as u16;
                    for shell in mgr.all_shells_mut() {
                        if shell.enabled {
                            shell.set_term_size(adjusted_cols, rows);
                        }
                    }
                }
            }
        }

        if mgr.all_terminated() {
            console.output(b"").await;
            break;
        }

        // Request input when all shells idle, or after a drain timeout while running
        if interactive && !input_requested {
            let (awaiting, _) = mgr.count_awaited_processes();
            if awaiting == 0 {
                // All shells idle: flush and prompt immediately
                drain_deadline = None;
                let max_name_len = display_names.max_display_name_length;
                for shell in mgr.all_shells_mut() {
                    shell.print_unfinished_line(&mut console, max_name_len).await;
                }

                let (idle, running, pending, dead, disabled) = mgr.count_by_state();
                let prompt = build_prompt(idle, running, pending, dead, disabled, use_color);
                let visible = build_prompt(idle, running, pending, dead, disabled, false);
                console.set_last_status_length(visible.len());
                if let Some(ref tx) = input_req_tx {
                    let _ = tx.send(InputRequest::ReadLine { prompt }).await;
                    input_requested = true;
                }
            } else if drain_deadline.is_none() {
                // Shells running, no timer yet: start drain timer
                drain_deadline = Some(Instant::now() + DRAIN_TIMEOUT);
            }
        }

        tokio::select! {
            Some(shell_evt) = shell_event_rx.recv() => {
                match shell_evt {
                    ShellEvent::Data { id, data } => {
                        // Reset drain timer: new data arrived, wait for output to settle
                        if drain_deadline.is_some() {
                            drain_deadline = Some(Instant::now() + DRAIN_TIMEOUT);
                        }
                        let max_name_len = display_names.max_display_name_length;
                        let abort = args.abort_errors;
                        if let Some(shell) = mgr.get_shell_mut(id) {
                            if let Some(new_name) = shell.handle_data(&data, &mut console, max_name_len, interactive, abort).await {
                                let new_name_str = String::from_utf8_lossy(&new_name).to_string();
                                let prev = shell.display_name.clone();
                                if let Some(name) = display_names.change(Some(&prev), Some(&new_name_str)) {
                                    shell.display_name = name;
                                }
                            }
                        }
                    }
                    ShellEvent::Closed { id, exit_code: code } => {
                        // Shell state changed; let top-of-loop logic re-evaluate
                        drain_deadline = None;
                        exit_code = std::cmp::max(exit_code, code);
                        let max_name_len = display_names.max_display_name_length;
                        if let Some(shell) = mgr.get_shell_mut(id) {
                            if code != 0 && interactive {
                                let msg = format!("Error talking to {}\n", shell.display_name);
                                console.output(msg.as_bytes()).await;
                            }
                            shell.disconnect(&mut console, max_name_len, args.abort_errors).await;
                            if interactive {
                                display_names.set_enabled(&shell.display_name, false);
                            }
                        }
                    }
                }
            }
            resp = async {
                if let Some(ref mut rx) = input_resp_rx {
                    rx.recv().await
                } else {
                    std::future::pending::<Option<InputEvent>>().await
                }
            } => {
                input_requested = false;
                if let Some(evt) = resp {
                    match evt {
                        InputEvent::Line(line) => {
                            console.log(format!("> {}\n", line).as_bytes()).await;

                            if let Some(cmd_line) = line.strip_prefix(':') {
                                let result = control_commands::dispatch(
                                    cmd_line,
                                    &mut mgr,
                                    &mut console,
                                    &mut display_names,
                                    interactive,
                                    &args,
                                ).await;
                                match result {
                                    control_commands::CmdResult::Ok => {}
                                    control_commands::CmdResult::Quit => break,
                                    control_commands::CmdResult::Error(msg) => {
                                        console.output(format!("{}\n", msg).as_bytes()).await;
                                    }
                                    control_commands::CmdResult::AddHosts(new_hosts) => {
                                        for h in &new_hosts {
                                            if let Err(e) = spawn_shell(
                                                h, &args, &command, &password,
                                                &mut mgr, &mut display_names,
                                                &persistent_shell_tx,
                                            ) {
                                                console.output(format!("{:#}\n", e).as_bytes()).await;
                                            }
                                        }
                                    }
                                }
                            } else if let Some(cmd) = line.strip_prefix('!') {
                                match tokio::process::Command::new("/bin/sh")
                                    .arg("-c")
                                    .arg(cmd)
                                    .status()
                                    .await
                                {
                                    Ok(s) => {
                                        if let Some(code) = s.code() {
                                            if code > 0 {
                                                console.output(
                                                    format!("Child returned {}\n", code).as_bytes(),
                                                ).await;
                                            }
                                        } else {
                                            console.output(b"Child was terminated by signal\n").await;
                                        }
                                    }
                                    Err(e) => {
                                        console.output(format!("Error: {}\n", e).as_bytes()).await;
                                    }
                                }
                            } else if line == "\x04" {
                                for shell in mgr.all_shells_mut() {
                                    if shell.enabled && shell.state != ShellState::Dead {
                                        shell.dispatch_command(b"\x04").await;
                                    }
                                }
                            } else {
                                let cmd = format!("{}\n", line);
                                for shell in mgr.all_shells_mut() {
                                    shell.dispatch_command(cmd.as_bytes()).await;
                                }
                            }

                            if let Ok(mut cs) = completion_state.write() {
                                cs.update_from_manager(&mgr);
                                if !line.starts_with(':') {
                                    cs.add_history_words(&line);
                                }
                            }
                        }
                        InputEvent::Eof => break,
                        InputEvent::Interrupted => {
                            // Forward Ctrl-C to running shells
                            for shell in mgr.all_shells_mut() {
                                if shell.enabled && shell.state == ShellState::Running {
                                    shell.write_to_pty(b"\x03");
                                }
                            }
                        }
                    }
                }
            }
            Some(sig) = signal_rx.recv() => {
                next_signal = Some(sig);
            }
            _ = tokio::time::sleep(DRAIN_TIMEOUT), if drain_deadline.is_some() && !input_requested => {
                // Drain timer fired: flush partial output and show prompt
                drain_deadline = None;
                let max_name_len = display_names.max_display_name_length;
                for shell in mgr.all_shells_mut() {
                    shell.print_unfinished_line(&mut console, max_name_len).await;
                }

                let (idle, running, pending, dead, disabled) = mgr.count_by_state();
                let prompt = build_prompt(idle, running, pending, dead, disabled, use_color);
                let visible = build_prompt(idle, running, pending, dead, disabled, false);
                console.set_last_status_length(visible.len());
                if let Some(ref tx) = input_req_tx {
                    let _ = tx.send(InputRequest::ReadLine { prompt }).await;
                    input_requested = true;
                }
            }
            else => break,
        }
    }

    // Cleanup
    kill_all(&mgr);

    if let Some(tx) = input_req_tx {
        let _ = tx.send(InputRequest::Shutdown).await;
    }

    if let Some(ref attrs) = saved_termios {
        nix::sys::termios::tcsetattr(std::io::stdin().as_fd(), nix::sys::termios::SetArg::TCSADRAIN, attrs).ok();
    }

    console.output(b"").await;
    std::process::exit(exit_code);
}

fn build_prompt(idle: usize, running: usize, pending: usize, dead: usize, disabled: usize, color: bool) -> String {
    let mut status_parts: Vec<String> = Vec::new();

    if idle > 0 {
        if color {
            status_parts.push(format!("{} {}", "●".green(), idle));
        } else {
            status_parts.push(format!("● {}", idle));
        }
    }
    if running > 0 {
        if color {
            status_parts.push(format!("{} {}", "◉".yellow(), running));
        } else {
            status_parts.push(format!("◉ {}", running));
        }
    }
    if pending > 0 {
        if color {
            status_parts.push(format!("{} {}", "◌".blue(), pending));
        } else {
            status_parts.push(format!("◌ {}", pending));
        }
    }
    if dead > 0 {
        if color {
            status_parts.push(format!("{} {}", "✕".red(), dead));
        } else {
            status_parts.push(format!("✕ {}", dead));
        }
    }
    if disabled > 0 {
        if color {
            status_parts.push(format!("{} {}", "○".bright_black(), disabled));
        } else {
            status_parts.push(format!("○ {}", disabled));
        }
    }

    let status = status_parts.join(" ");
    if color {
        format!("mash [{}] {}{}{} ", status, "❯".red(), "❯".yellow(), "❯".green())
    } else {
        format!("mash [{}] ❯❯❯ ", status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_prompt_all_idle() {
        let p = build_prompt(5, 0, 0, 0, 0, false);
        assert!(p.contains("● 5"));
        assert!(p.starts_with("mash ["));
        assert!(p.ends_with("❯❯❯ "));
    }

    #[test]
    fn test_build_prompt_mixed_states() {
        let p = build_prompt(3, 1, 2, 0, 0, false);
        assert!(p.contains("● 3"));
        assert!(p.contains("◉ 1"));
        assert!(p.contains("◌ 2"));
        assert!(!p.contains("✕"));
        assert!(!p.contains("○"));
    }

    #[test]
    fn test_build_prompt_dead_and_disabled() {
        let p = build_prompt(0, 0, 0, 2, 1, false);
        assert!(p.contains("✕ 2"));
        assert!(p.contains("○ 1"));
        assert!(!p.contains("●"));
    }

    #[test]
    fn test_build_prompt_all_states() {
        let p = build_prompt(1, 2, 3, 4, 5, false);
        assert!(p.contains("● 1"));
        assert!(p.contains("◉ 2"));
        assert!(p.contains("◌ 3"));
        assert!(p.contains("✕ 4"));
        assert!(p.contains("○ 5"));
    }

    #[test]
    fn test_build_prompt_colored_has_ansi() {
        let p = build_prompt(3, 0, 0, 0, 0, true);
        // Should contain ANSI escape codes
        assert!(p.contains("\x1b["));
        assert!(p.contains("mash"));
    }

    #[test]
    fn test_build_prompt_no_color_no_ansi() {
        let p = build_prompt(3, 0, 0, 0, 0, false);
        assert!(!p.contains("\x1b["));
    }
}
