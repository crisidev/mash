use std::borrow::Cow;
use std::os::fd::AsFd;

use crate::cli::Args;
use crate::console::Console;
use crate::display_names::DisplayNameRegistry;
use crate::host_syntax::expand_syntax;
use crate::shell::ShellState;
use crate::shell_manager::ShellManager;

pub(crate) enum CmdResult {
    Ok,
    Quit,
    Error(String),
    AddHosts(Vec<String>),
}

pub(crate) async fn dispatch(
    line: &str,
    mgr: &mut ShellManager,
    console: &mut Console,
    display_names: &mut DisplayNameRegistry,
    interactive: bool,
    _args: &Args,
) -> CmdResult {
    if line.is_empty() {
        return CmdResult::Ok;
    }

    let (cmd_name, params) = match line.split_once(char::is_whitespace) {
        Some((cmd, rest)) => (cmd, rest),
        None => (line, ""),
    };

    match cmd_name {
        "help" => do_help(console).await,
        "list" => do_list(params, mgr, console).await,
        "quit" => CmdResult::Quit,
        "enable" => do_enable(params, mgr, console, display_names, interactive).await,
        "disable" => do_disable(params, mgr, console, display_names, interactive).await,
        "reconnect" => do_reconnect(params, mgr, console, display_names).await,
        "add" => do_add(params),
        "purge" => do_purge(params, mgr, console, display_names).await,
        "rename" => do_rename(params, mgr).await,
        "send_ctrl" => do_send_ctrl(params, mgr, console).await,
        "reset_prompt" => do_reset_prompt(params, mgr, console).await,
        "chdir" => do_chdir(params, console).await,
        "hide_password" => do_hide_password(mgr, console).await,
        "set_debug" => do_set_debug(params, mgr, console).await,
        "export_vars" => do_export_vars(mgr).await,
        "set_log" => do_set_log(params, console).await,
        "show_read_buffer" => do_show_read_buffer(params, mgr, console).await,
        _ => CmdResult::Error(format!("Unknown control command: {}. Type :help for usage.", cmd_name)),
    }
}

pub(crate) fn list_command_names() -> Vec<&'static str> {
    COMMANDS.iter().map(|c| c.name).collect()
}

struct CommandInfo {
    name: &'static str,
    args: &'static str,
    description: &'static str,
}

const COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "help",
        args: "",
        description: "Show this help message",
    },
    CommandInfo {
        name: "list",
        args: "[PATTERN]",
        description: "List remote shells and their status",
    },
    CommandInfo {
        name: "quit",
        args: "",
        description: "Close all connections and exit",
    },
    CommandInfo {
        name: "enable",
        args: "[PATTERN]",
        description: "Enable matching shells (or toggle if all match)",
    },
    CommandInfo {
        name: "disable",
        args: "[PATTERN]",
        description: "Disable matching shells (or toggle if all match)",
    },
    CommandInfo {
        name: "reconnect",
        args: "[PATTERN]",
        description: "Reconnect dead shells",
    },
    CommandInfo {
        name: "add",
        args: "HOST...",
        description: "Add new SSH connections",
    },
    CommandInfo {
        name: "purge",
        args: "[PATTERN]",
        description: "Remove disabled shells",
    },
    CommandInfo {
        name: "rename",
        args: "NAME",
        description: "Rename enabled shells (supports shell expansion)",
    },
    CommandInfo {
        name: "send_ctrl",
        args: "LETTER [PATTERN]",
        description: "Send a control character (e.g. :send_ctrl c)",
    },
    CommandInfo {
        name: "reset_prompt",
        args: "[PATTERN]",
        description: "Re-send the prompt initialization string",
    },
    CommandInfo {
        name: "chdir",
        args: "[PATH]",
        description: "Change the local working directory",
    },
    CommandInfo {
        name: "hide_password",
        args: "",
        description: "Disable echo, debug, and logging for password entry",
    },
    CommandInfo {
        name: "set_debug",
        args: "y|n [PATTERN]",
        description: "Enable or disable debug output per shell",
    },
    CommandInfo {
        name: "export_vars",
        args: "",
        description: "Set MASH_RANK/NAME/NR_SHELLS on each shell",
    },
    CommandInfo {
        name: "set_log",
        args: "[PATH]",
        description: "Set or disable the log file",
    },
    CommandInfo {
        name: "show_read_buffer",
        args: "[PATTERN]",
        description: "Show buffered output from shell startup",
    },
];

async fn do_help(console: &mut Console) -> CmdResult {
    use owo_colors::OwoColorize;

    let mut out = String::new();

    out.push_str(&format!(
        "{}\n\n",
        "mash - control multiple SSH sessions from one prompt".bold()
    ));

    out.push_str(&format!("{}\n", "Input modes:".yellow().bold()));
    out.push_str(&format!(
        "  {}        Send to all enabled remote shells\n",
        "<command>".cyan()
    ));
    out.push_str(&format!(
        "  {} Run a mash control command (see below)\n",
        ":command [args]".cyan()
    ));
    out.push_str(&format!("  {}         Run a local shell command\n", "!command".cyan()));
    out.push_str(&format!(
        "  {}           Send EOF to all remote shells\n\n",
        "Ctrl-D".cyan()
    ));

    out.push_str(&format!("{}\n", "Prompt indicators:".yellow().bold()));
    out.push_str(&format!(
        "  {} idle  {} running  {} pending  {} dead  {} disabled\n\n",
        "●".green(),
        "◉".yellow(),
        "◌".blue(),
        "✕".red(),
        "○".bright_black()
    ));

    out.push_str(&format!("{}\n", "Control commands:".yellow().bold()));

    let max_name = COMMANDS.iter().map(|c| c.name.len() + c.args.len()).max().unwrap_or(0) + 2;
    for cmd in COMMANDS {
        let usage = if cmd.args.is_empty() {
            format!(":{}", cmd.name)
        } else {
            format!(":{} {}", cmd.name, cmd.args.dimmed())
        };
        // Pad based on visible length (without ANSI codes)
        let visible = if cmd.args.is_empty() {
            format!(":{}", cmd.name)
        } else {
            format!(":{} {}", cmd.name, cmd.args)
        };
        let padding = max_name + 1 - visible.len().min(max_name + 1);
        out.push_str(&format!(
            "  {}{} {}\n",
            usage,
            " ".repeat(padding),
            cmd.description.dimmed()
        ));
    }

    out.push_str(&format!(
        "\n{} can use {} and {} wildcards to match shell names or last output.\n",
        "PATTERN".cyan(),
        "*".bold(),
        "?".bold()
    ));
    out.push_str(&format!("Omitting {} selects all shells.\n", "PATTERN".cyan()));

    console.output(out.as_bytes()).await;
    CmdResult::Ok
}

async fn selected_shells_indices(command: &str, mgr: &ShellManager, console: &mut Console) -> Vec<usize> {
    let _ids = mgr.shell_ids();
    let shells = mgr.all_shells();

    if command.is_empty() || command == "*" {
        return (0..shells.len()).collect();
    }

    let mut selected = Vec::new();
    let mut selected_set = std::collections::HashSet::new();

    for pattern in command.split_whitespace() {
        let expanded: Vec<String> = expand_syntax(pattern);
        let mut found = false;
        for expanded_pattern in &expanded {
            let glob_pat = glob::Pattern::new(expanded_pattern);
            for (idx, shell) in shells.iter().enumerate() {
                if !selected_set.contains(&idx) {
                    let matches = match &glob_pat {
                        Ok(p) => {
                            p.matches(&shell.display_name)
                                || p.matches(&String::from_utf8_lossy(&shell.last_printed_line))
                        }
                        Err(_) => {
                            shell.display_name == *expanded_pattern
                                || String::from_utf8_lossy(&shell.last_printed_line) == *expanded_pattern
                        }
                    };
                    if matches {
                        found = true;
                        selected_set.insert(idx);
                        selected.push(idx);
                    }
                }
            }
        }
        if !found && !shells.is_empty() {
            console.output(format!("{} not found\n", pattern).as_bytes()).await;
        }
    }

    selected
}

async fn do_list(params: &str, mgr: &ShellManager, console: &mut Console) -> CmdResult {
    let shells = mgr.all_shells();
    let indices = selected_shells_indices(params, mgr, console).await;
    let info_list: Vec<Vec<Vec<u8>>> = indices.iter().map(|&i| shells[i].get_info()).collect();
    let formatted = ShellManager::format_info(&info_list);
    for line in formatted {
        console.output(&line).await;
    }
    CmdResult::Ok
}

async fn do_enable(
    params: &str,
    mgr: &mut ShellManager,
    console: &mut Console,
    display_names: &mut DisplayNameRegistry,
    interactive: bool,
) -> CmdResult {
    toggle_shells(params, true, mgr, console, display_names, interactive).await;
    CmdResult::Ok
}

async fn do_disable(
    params: &str,
    mgr: &mut ShellManager,
    console: &mut Console,
    display_names: &mut DisplayNameRegistry,
    interactive: bool,
) -> CmdResult {
    toggle_shells(params, false, mgr, console, display_names, interactive).await;
    CmdResult::Ok
}

async fn toggle_shells(
    command: &str,
    enable: bool,
    mgr: &mut ShellManager,
    console: &mut Console,
    display_names: &mut DisplayNameRegistry,
    interactive: bool,
) {
    let indices = selected_shells_indices(command, mgr, console).await;
    let shells = mgr.all_shells();

    // Check if the toggle would have no effect
    if !command.is_empty() && command != "*" && !indices.is_empty() {
        let all_same = indices
            .iter()
            .all(|&i| shells[i].state == ShellState::Dead || shells[i].enabled == enable);
        if all_same {
            // Toggle all others instead
            let all_indices: Vec<usize> = (0..shells.len()).collect();
            drop(shells);
            for &i in &all_indices {
                let shells = mgr.all_shells();
                let shell_id = shells[i].id;
                let shell_name = shells[i].display_name.clone();
                let shell_state = shells[i].state;
                drop(shells);
                if shell_state != ShellState::Dead {
                    if let Some(shell) = mgr.get_shell_mut(shell_id) {
                        if shell.enabled == enable && interactive {
                            display_names.set_enabled(&shell_name, !enable);
                        }
                        shell.enabled = !enable;
                    }
                }
            }
            return;
        }
    }
    drop(shells);

    for &i in &indices {
        let shells = mgr.all_shells();
        let shell_id = shells[i].id;
        let shell_name = shells[i].display_name.clone();
        let shell_state = shells[i].state;
        drop(shells);
        if shell_state != ShellState::Dead {
            if let Some(shell) = mgr.get_shell_mut(shell_id) {
                if shell.enabled != enable && interactive {
                    display_names.set_enabled(&shell_name, enable);
                }
                shell.enabled = enable;
            }
        }
    }
}

async fn do_reconnect(
    params: &str,
    mgr: &mut ShellManager,
    console: &mut Console,
    display_names: &mut DisplayNameRegistry,
) -> CmdResult {
    let indices = selected_shells_indices(params, mgr, console).await;
    let shells = mgr.all_shells();
    let hosts: Vec<String> = indices
        .iter()
        .filter(|&&i| shells[i].state == ShellState::Dead)
        .map(|&i| {
            let h = shells[i].hostname.clone();
            let p = shells[i].port.clone();
            if p == "22" { h } else { format!("{}:{}", h, p) }
        })
        .collect();

    // Remove dead shells
    let to_remove: Vec<_> = indices
        .iter()
        .filter(|&&i| shells[i].state == ShellState::Dead)
        .map(|&i| shells[i].id)
        .collect();
    drop(shells);

    for id in to_remove {
        if let Some(shell) = mgr.get_shell(id) {
            display_names.change(Some(&shell.display_name.clone()), None);
        }
        mgr.remove_shell(id);
    }

    if hosts.is_empty() {
        CmdResult::Ok
    } else {
        CmdResult::AddHosts(hosts)
    }
}

fn do_add(params: &str) -> CmdResult {
    let hosts: Vec<String> = params.split_whitespace().map(String::from).collect();
    if hosts.is_empty() {
        CmdResult::Error("Expected at least one hostname".into())
    } else {
        CmdResult::AddHosts(hosts)
    }
}

async fn do_purge(
    params: &str,
    mgr: &mut ShellManager,
    console: &mut Console,
    display_names: &mut DisplayNameRegistry,
) -> CmdResult {
    let indices = selected_shells_indices(params, mgr, console).await;
    let shells = mgr.all_shells();
    let to_remove: Vec<_> = indices
        .iter()
        .filter(|&&i| !shells[i].enabled)
        .map(|&i| (shells[i].id, shells[i].display_name.clone()))
        .collect();
    drop(shells);

    for (id, name) in to_remove {
        display_names.change(Some(&name), None);
        if let Some(shell) = mgr.get_shell_mut(id) {
            shell
                .disconnect(console, display_names.max_display_name_length, false)
                .await;
        }
        mgr.remove_shell(id);
    }

    CmdResult::Ok
}

async fn do_rename(params: &str, mgr: &mut ShellManager) -> CmdResult {
    let name = params.trim().as_bytes();
    for shell in mgr.all_shells_mut() {
        if shell.enabled {
            if name.is_empty() {
                // Reset to hostname
                // This would need display_names access - simplified version
            } else {
                // Use shell-side expansion via echo
                let (r1, r2) = shell.callbacks.add(
                    b"rename",
                    crate::callbacks::CallbackAction::Rename { new_name: Vec::new() },
                    false,
                );
                let cmd = format!(
                    "/bin/echo \"{}\"\"{}\" {}\n",
                    String::from_utf8_lossy(&r1),
                    String::from_utf8_lossy(&r2),
                    String::from_utf8_lossy(name),
                );
                shell.dispatch_command(cmd.as_bytes()).await;
            }
        }
    }
    CmdResult::Ok
}

async fn do_send_ctrl(params: &str, mgr: &mut ShellManager, console: &mut Console) -> CmdResult {
    let mut split = params.split_whitespace();
    let letter = match split.next() {
        Some(l) => l,
        None => return CmdResult::Error("Expected at least a letter".into()),
    };
    if letter.len() != 1 {
        return CmdResult::Error(format!("Expected a single letter, got: {}", letter));
    }
    let ctrl_char = letter.to_ascii_lowercase().as_bytes()[0] - b'a' + 1;
    let remaining: String = split.collect::<Vec<&str>>().join(" ");
    let indices = selected_shells_indices(&remaining, mgr, console).await;
    let shells = mgr.all_shells();
    let ids: Vec<_> = indices
        .iter()
        .filter(|&&i| shells[i].enabled)
        .map(|&i| shells[i].id)
        .collect();
    drop(shells);
    for id in ids {
        if let Some(shell) = mgr.get_shell_mut(id) {
            shell.dispatch_write(&[ctrl_char]);
        }
    }
    CmdResult::Ok
}

async fn do_reset_prompt(params: &str, mgr: &mut ShellManager, console: &mut Console) -> CmdResult {
    let indices = selected_shells_indices(params, mgr, console).await;
    let shells = mgr.all_shells();
    let ids: Vec<_> = indices.iter().map(|&i| shells[i].id).collect();
    drop(shells);
    for id in ids {
        if let Some(shell) = mgr.get_shell_mut(id) {
            shell.rebuild_init_string();
            let init = shell.init_string.clone();
            shell.dispatch_command(&init).await;
        }
    }
    CmdResult::Ok
}

async fn do_chdir(params: &str, console: &mut Console) -> CmdResult {
    let path = params.trim();
    let path = if path.is_empty() { "~" } else { path };
    let expanded = shellexpand::full(path).unwrap_or(Cow::Borrowed(path)).to_string();
    if let Err(e) = std::env::set_current_dir(&expanded) {
        console.output(format!("{}\n", e).as_bytes()).await;
    }
    CmdResult::Ok
}

async fn do_hide_password(mgr: &mut ShellManager, console: &mut Console) -> CmdResult {
    let mut warned = false;
    for shell in mgr.all_shells_mut() {
        if shell.enabled && shell.debug {
            shell.debug = false;
            if !warned {
                console
                    .output(b"Debugging disabled to avoid displaying passwords\n")
                    .await;
                warned = true;
            }
        }
    }

    if console.has_log() {
        console.output(b"Logging disabled to avoid writing passwords\n").await;
        console.disable_log();
    }

    // Disable terminal echo
    let stdin = std::io::stdin();
    if let Ok(mut attrs) = nix::sys::termios::tcgetattr(stdin.as_fd()) {
        attrs.local_flags.remove(nix::sys::termios::LocalFlags::ECHO);
        let _ = nix::sys::termios::tcsetattr(stdin.as_fd(), nix::sys::termios::SetArg::TCSANOW, &attrs);
    }

    CmdResult::Ok
}

async fn do_set_debug(params: &str, mgr: &mut ShellManager, console: &mut Console) -> CmdResult {
    let mut split = params.split_whitespace();
    let letter = match split.next() {
        Some(l) => l,
        None => return CmdResult::Error("Expected at least a letter".into()),
    };
    let debug = match letter.to_lowercase().as_str() {
        "y" => true,
        "n" => false,
        _ => return CmdResult::Error(format!("Expected 'y' or 'n', got: {}", letter)),
    };

    let remaining: String = split.collect::<Vec<&str>>().join(" ");
    let indices = selected_shells_indices(&remaining, mgr, console).await;
    let shells = mgr.all_shells();
    let ids: Vec<_> = indices.iter().map(|&i| shells[i].id).collect();
    drop(shells);
    for id in ids {
        if let Some(shell) = mgr.get_shell_mut(id) {
            shell.debug = debug;
        }
    }

    CmdResult::Ok
}

async fn do_export_vars(mgr: &mut ShellManager) -> CmdResult {
    let mut rank = 0usize;
    let shells: Vec<_> = mgr
        .all_shells()
        .iter()
        .filter(|s| s.enabled)
        .map(|s| (s.id, s.hostname.clone(), s.display_name.clone()))
        .collect();
    let total = shells.len();

    for (id, hostname, display_name) in &shells {
        if let Some(shell) = mgr.get_shell_mut(*id) {
            let cmd = format!(
                "export MASH_RANK={} MASH_NAME={} MASH_DISPLAY_NAME={}\n",
                rank,
                shell_words::quote(hostname),
                shell_words::quote(display_name),
            );
            shell.dispatch_command(cmd.as_bytes()).await;
            rank += 1;
        }
    }

    for (id, _, _) in &shells {
        if let Some(shell) = mgr.get_shell_mut(*id) {
            let cmd = format!("export MASH_NR_SHELLS={}\n", total);
            shell.dispatch_command(cmd.as_bytes()).await;
        }
    }

    CmdResult::Ok
}

async fn do_set_log(params: &str, console: &mut Console) -> CmdResult {
    let path = params.trim();
    if path.is_empty() {
        console.disable_log();
        console.output(b"Logging disabled\n").await;
    } else {
        console.set_log_file(Some(path)).await;
    }
    CmdResult::Ok
}

async fn do_show_read_buffer(params: &str, mgr: &mut ShellManager, console: &mut Console) -> CmdResult {
    let indices = selected_shells_indices(params, mgr, console).await;
    let shells = mgr.all_shells();
    let ids: Vec<_> = indices.iter().map(|&i| shells[i].id).collect();
    let max_name_len = shells
        .iter()
        .filter(|s| s.enabled)
        .map(|s| s.display_name.len())
        .max()
        .unwrap_or(0);
    drop(shells);

    for id in ids {
        if let Some(shell) = mgr.get_shell_mut(id) {
            if !shell.read_in_state_not_started.is_empty() {
                let data = std::mem::take(&mut shell.read_in_state_not_started);
                shell.print_lines(&data, console, max_name_len).await;
            }
        }
    }
    CmdResult::Ok
}
