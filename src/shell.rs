use std::os::fd::{AsFd, AsRawFd, OwnedFd};

use nix::pty::Winsize;
use owo_colors::{AnsiColors, OwoColorize, Style};

use crate::callbacks::{CallbackAction, CallbackRegistry};
use crate::console::Console;

nix::ioctl_write_ptr_bad!(set_winsize, nix::libc::TIOCSWINSZ, Winsize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ShellId(pub(crate) usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellState {
    NotStarted,
    Idle,
    Running,
    Terminated,
    Dead,
}

impl ShellState {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            ShellState::NotStarted => "not_started",
            ShellState::Idle => "idle",
            ShellState::Running => "running",
            ShellState::Terminated => "terminated",
            ShellState::Dead => "dead",
        }
    }
}

const COLORS: &[AnsiColors] = &[
    AnsiColors::BrightBlack,
    AnsiColors::Red,
    AnsiColors::Green,
    AnsiColors::Yellow,
    AnsiColors::Blue,
    AnsiColors::Magenta,
    AnsiColors::Cyan,
    AnsiColors::Default,
];

pub(crate) struct RemoteShell {
    pub(crate) id: ShellId,
    pub(crate) hostname: String,
    pub(crate) port: String,
    pub(crate) display_name: String,
    pub(crate) enabled: bool,
    pub(crate) state: ShellState,
    pub(crate) pid: i32,
    pub(crate) master_fd: OwnedFd,
    pub(crate) color_style: Option<Style>,
    pub(crate) debug: bool,
    pub(crate) read_buffer: Vec<u8>,
    pub(crate) write_buffer: Vec<u8>,
    pub(crate) last_printed_line: Vec<u8>,
    pub(crate) read_in_state_not_started: Vec<u8>,
    pub(crate) init_string: Vec<u8>,
    pub(crate) init_string_sent: bool,
    pub(crate) command: Option<String>,
    pub(crate) password: Option<String>,
    pub(crate) callbacks: CallbackRegistry,
}

impl RemoteShell {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: ShellId,
        hostname: String,
        port: String,
        display_name: String,
        pid: i32,
        master_fd: OwnedFd,
        debug: bool,
        command: Option<String>,
        password: Option<String>,
        color_idx: usize,
        use_color: bool,
    ) -> Self {
        let color_style = if use_color {
            let color = COLORS[color_idx % COLORS.len()];
            Some(Style::new().color(color).bold())
        } else {
            None
        };

        let mut callbacks = CallbackRegistry::new();
        let init_string = Self::build_init_string(id, &mut callbacks);

        Self {
            id,
            hostname,
            port,
            display_name,
            enabled: true,
            state: ShellState::NotStarted,
            pid,
            master_fd,
            color_style,
            debug,
            read_buffer: Vec::new(),
            write_buffer: Vec::new(),
            last_printed_line: Vec::new(),
            read_in_state_not_started: Vec::new(),
            init_string,
            init_string_sent: false,
            command,
            password,
            callbacks,
        }
    }

    fn build_init_string(_id: ShellId, callbacks: &mut CallbackRegistry) -> Vec<u8> {
        let mut init = Vec::new();
        // Line 1: disable ZLE so subsequent lines are not mangled by zsh line editor.
        init.extend_from_slice(b"unsetopt zle 2>/dev/null\n");
        // Line 2: everything else on one line (ZLE is off so no wrapping/mangling).
        // TTY config, clear zsh hooks/themes, set prompt.
        init.extend_from_slice(b"stty -echo -onlcr -ctlecho;bind \"set enable-bracketed-paste off\" 2>/dev/null;");
        // Clear zsh hook arrays and functions (unset is POSIX-safe, unlike var=() which breaks dash/sh)
        init.extend_from_slice(b"unset precmd_functions preexec_functions chpwd_functions 2>/dev/null;");
        // Remove hook functions: unfunction (zsh), unset -f (bash)
        init.extend_from_slice(b"unfunction precmd preexec 2>/dev/null;unset -f precmd preexec 2>/dev/null;");
        init.extend_from_slice(b"prompt off 2>/dev/null;");
        // Disable zsh's partial-line marker (the % at end of output without trailing newline)
        // unsetopt/PROMPT_EOL_MARK are zsh-only but harmless on other shells (silent no-op or unused var)
        init.extend_from_slice(b"unsetopt PROMPT_CR PROMPT_SP 2>/dev/null;PROMPT_EOL_MARK=;");
        init.extend_from_slice(b"PS2=;RPS1=;RPROMPT=;PROMPT_COMMAND=;TERM=ansi;unset HISTFILE;");

        let (p1, p2) = callbacks.add(b"prompt", CallbackAction::SeenPrompt, true);
        init.extend_from_slice(b"PS1=\"");
        init.extend_from_slice(&p1);
        init.extend_from_slice(b"\"\"");
        init.extend_from_slice(&p2);
        init.extend_from_slice(b"\n\"\n");
        init
    }

    pub(crate) fn rebuild_init_string(&mut self) {
        self.init_string = Self::build_init_string(self.id, &mut self.callbacks);
    }

    async fn change_state(&mut self, new_state: ShellState, console: Option<&mut Console>) {
        if new_state != self.state {
            if self.debug {
                if let Some(c) = console {
                    self.print_debug(format!("state => {}", new_state.name()).as_bytes(), c)
                        .await;
                }
            }
            if self.state == ShellState::NotStarted {
                self.read_in_state_not_started.clear();
            }
            self.state = new_state;
        }
    }

    pub(crate) fn write_to_pty(&self, data: &[u8]) {
        let _ = nix::unistd::write(self.master_fd.as_fd(), data);
    }

    pub(crate) fn dispatch_write(&mut self, buf: &[u8]) -> bool {
        if self.state != ShellState::Dead && self.enabled {
            self.write_to_pty(buf);
            true
        } else {
            false
        }
    }

    pub(crate) async fn dispatch_command(&mut self, command: &[u8]) {
        if self.dispatch_write(command) && self.state == ShellState::Idle {
            self.change_state(ShellState::Running, None).await;
        }
    }

    pub(crate) async fn disconnect(&mut self, console: &mut Console, max_name_len: usize, _abort_error: bool) {
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(-self.pid), nix::sys::signal::Signal::SIGKILL);
        self.read_buffer.clear();
        self.write_buffer.clear();
        self.enabled = false;

        if !self.read_in_state_not_started.is_empty() {
            let data = std::mem::take(&mut self.read_in_state_not_started);
            self.print_lines(&data, console, max_name_len).await;
        }

        self.change_state(ShellState::Dead, Some(console)).await;
    }

    pub(crate) async fn print_lines(&mut self, lines: &[u8], console: &mut Console, max_name_len: usize) {
        // Strip leading/trailing newlines, collapse double newlines
        let cleaned = strip_newlines(lines);
        if cleaned.is_empty() {
            return;
        }

        let indent = if max_name_len >= self.display_name.len() {
            max_name_len - self.display_name.len()
        } else {
            0
        };

        let log_prefix = format!("{}{} : ", self.display_name, " ".repeat(indent));
        let console_prefix = match self.color_style {
            Some(style) => format!("{}", log_prefix.style(style)),
            None => log_prefix.clone(),
        };

        let log_prefix_bytes = log_prefix.as_bytes();
        let console_prefix_bytes = console_prefix.as_bytes();

        let mut console_data = Vec::new();
        let mut log_data = Vec::new();

        console_data.extend_from_slice(console_prefix_bytes);
        log_data.extend_from_slice(log_prefix_bytes);

        for &b in &cleaned {
            if b == b'\n' {
                console_data.push(b'\n');
                console_data.extend_from_slice(console_prefix_bytes);
                log_data.push(b'\n');
                log_data.extend_from_slice(log_prefix_bytes);
            } else {
                console_data.push(b);
                log_data.push(b);
            }
        }
        console_data.push(b'\n');
        log_data.push(b'\n');

        console.output_with_log(&console_data, Some(&log_data)).await;

        // Track last printed line
        if let Some(pos) = cleaned.iter().rposition(|&b| b == b'\n') {
            self.last_printed_line = cleaned[pos + 1..].to_vec();
        } else {
            self.last_printed_line = cleaned;
        }
    }

    /// Process incoming data. Returns Some(new_name) if a rename callback was triggered.
    pub(crate) async fn handle_data(
        &mut self,
        new_data: &[u8],
        console: &mut Console,
        max_name_len: usize,
        interactive: bool,
        abort_error: bool,
    ) -> Option<Vec<u8>> {
        let mut pending_rename: Option<Vec<u8>> = None;
        if self.state == ShellState::Dead {
            return None;
        }

        if self.debug {
            self.print_debug(&[b"==> ", new_data].concat(), console).await;
        }

        self.read_buffer.extend_from_slice(new_data);

        // Fast path: running state, no callback markers, has newline
        if self.state == ShellState::Running && !self.callbacks.any_in(&self.read_buffer) {
            if let Some(last_nl) = self.read_buffer.iter().rposition(|&b| b == b'\n') {
                let to_print = self.read_buffer[..last_nl].to_vec();
                self.read_buffer = self.read_buffer[last_nl + 1..].to_vec();
                self.print_lines(&to_print, console, max_name_len).await;
                return None;
            }
        }

        // Check for password prompt in NOT_STARTED state
        if self.state == ShellState::NotStarted && self.password.is_some() {
            let lower: Vec<u8> = self.read_buffer.iter().map(|b| b.to_ascii_lowercase()).collect();
            if lower.windows(9).any(|w| w == b"password:") {
                if let Some(ref pw) = self.password {
                    let pw_cmd = format!("{}\n", pw);
                    self.write_to_pty(pw_cmd.as_bytes());
                    self.read_buffer.clear();
                    return None;
                }
            }
        }

        // Process line by line
        while let Some(lf_pos) = self.read_buffer.iter().position(|&b| b == b'\n') {
            let line = self.read_buffer[..lf_pos + 1].to_vec();
            self.read_buffer = self.read_buffer[lf_pos + 1..].to_vec();

            if let Some(action) = self.callbacks.process(&line) {
                match action {
                    CallbackAction::SeenPrompt => {
                        if interactive {
                            self.change_state(ShellState::Idle, Some(console)).await;
                        } else if let Some(cmd) = self.command.take() {
                            // Non-interactive: send command, then exit
                            let (p1, p2) = self.callbacks.add(b"real prompt ends", CallbackAction::None, true);
                            let ps1_cmd = format!(
                                "PS1=\"{}\"\"{}\n\"\n",
                                String::from_utf8_lossy(&p1),
                                String::from_utf8_lossy(&p2),
                            );
                            self.write_to_pty(ps1_cmd.as_bytes());
                            self.write_to_pty(cmd.as_bytes());
                            self.write_to_pty(b"exit 2>/dev/null\n");
                        }
                    }
                    CallbackAction::Rename { new_name } => {
                        if !new_name.is_empty() {
                            pending_rename = Some(new_name);
                        } else {
                            pending_rename = Some(self.hostname.as_bytes().to_vec());
                        }
                    }
                    CallbackAction::None => {}
                }
            } else if self.state == ShellState::Idle || self.state == ShellState::Running {
                self.print_lines(&line, console, max_name_len).await;
            } else if self.state == ShellState::NotStarted {
                self.read_in_state_not_started.extend_from_slice(&line);
                if line.windows(25).any(|w| w == b"The authenticity of host ") {
                    let trimmed = trim_ascii_bytes(&line);
                    let msg = [
                        trimmed,
                        b" Closing connection. Consider manually connecting or using ssh-keyscan.",
                    ]
                    .concat();
                    self.print_lines(&msg, console, max_name_len).await;
                    self.disconnect(console, max_name_len, abort_error).await;
                    return pending_rename;
                } else if line.windows(36).any(|w| w == b"REMOTE HOST IDENTIFICATION HAS CHANG") {
                    let msg =
                        b"Remote host identification has changed. Consider manually connecting or using ssh-keyscan.";
                    self.print_lines(msg, console, max_name_len).await;
                }
            }

            // Try fast path again after processing
            if self.state == ShellState::Running && !self.callbacks.any_in(&self.read_buffer) {
                if let Some(last_nl) = self.read_buffer.iter().rposition(|&b| b == b'\n') {
                    let to_print = self.read_buffer[..last_nl].to_vec();
                    self.read_buffer = self.read_buffer[last_nl + 1..].to_vec();
                    self.print_lines(&to_print, console, max_name_len).await;
                    return pending_rename;
                }
            }
        }

        // Send init string if not yet sent
        if self.state == ShellState::NotStarted && !self.init_string_sent {
            let init = self.init_string.clone();
            self.write_to_pty(&init);
            self.init_string_sent = true;
        }

        pending_rename
    }

    pub(crate) async fn print_unfinished_line(&mut self, console: &mut Console, max_name_len: usize) {
        if self.state == ShellState::Running && !self.read_buffer.is_empty() {
            let buf = std::mem::take(&mut self.read_buffer);
            if self.callbacks.process(&buf).is_none() {
                self.print_lines(&buf, console, max_name_len).await;
            }
        }
    }

    pub(crate) fn set_term_size(&self, cols: u16, rows: u16) {
        let wsz = Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe { set_winsize(self.master_fd.as_raw_fd(), &wsz) }.ok();
    }

    pub(crate) fn get_info(&self) -> Vec<Vec<u8>> {
        vec![
            self.display_name.as_bytes().to_vec(),
            if self.enabled {
                b"enabled".to_vec()
            } else {
                b"disabled".to_vec()
            },
            format!("{}:", self.state.name()).into_bytes(),
            self.last_printed_line.clone(),
        ]
    }

    async fn print_debug(&self, msg: &[u8], console: &mut Console) {
        let mut out = Vec::new();
        out.extend_from_slice(b"[dbg] ");
        out.extend_from_slice(self.display_name.as_bytes());
        out.extend_from_slice(b"[");
        out.extend_from_slice(self.state.name().as_bytes());
        out.extend_from_slice(b"]: ");
        out.extend_from_slice(msg);
        out.push(b'\n');
        console.output(&out).await;
    }
}

/// Trim ASCII whitespace from both ends of a byte slice.
fn trim_ascii_bytes(data: &[u8]) -> &[u8] {
    let start = data.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(data.len());
    let end = data
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|p| p + 1)
        .unwrap_or(start);
    &data[start..end]
}

fn strip_newlines(data: &[u8]) -> Vec<u8> {
    // Split into lines, drop empty/whitespace-only lines, rejoin
    let mut lines: Vec<&[u8]> = Vec::new();
    for line in data.split(|&b| b == b'\n') {
        if !line.iter().all(|b| b.is_ascii_whitespace()) {
            lines.push(line);
        }
    }
    // Strip leading/trailing empty entries and rejoin with newlines
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let mut result = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            result.push(b'\n');
        }
        result.extend_from_slice(line);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsFd;

    use super::*;
    use crate::console::Console;

    // --- trim_ascii_bytes tests ---

    #[test]
    fn test_trim_ascii_empty() {
        assert_eq!(trim_ascii_bytes(b""), b"");
    }

    #[test]
    fn test_trim_ascii_only_whitespace() {
        assert_eq!(trim_ascii_bytes(b"   \t\n  "), b"");
    }

    #[test]
    fn test_trim_ascii_no_whitespace() {
        assert_eq!(trim_ascii_bytes(b"hello"), b"hello");
    }

    #[test]
    fn test_trim_ascii_leading_trailing() {
        assert_eq!(trim_ascii_bytes(b"  hello world  "), b"hello world");
    }

    #[test]
    fn test_trim_ascii_tabs_and_newlines() {
        assert_eq!(trim_ascii_bytes(b"\t\nhello\n\t"), b"hello");
    }

    // --- strip_newlines tests ---

    #[test]
    fn test_strip_newlines_empty() {
        assert_eq!(strip_newlines(b""), b"");
    }

    #[test]
    fn test_strip_newlines_only_newlines() {
        assert_eq!(strip_newlines(b"\n\n\n"), b"");
    }

    #[test]
    fn test_strip_newlines_single_line() {
        assert_eq!(strip_newlines(b"hello"), b"hello");
    }

    #[test]
    fn test_strip_newlines_strips_leading_trailing() {
        assert_eq!(strip_newlines(b"\nhello\n"), b"hello");
    }

    #[test]
    fn test_strip_newlines_preserves_middle() {
        assert_eq!(strip_newlines(b"hello\nworld"), b"hello\nworld");
    }

    #[test]
    fn test_strip_newlines_removes_blank_lines() {
        assert_eq!(strip_newlines(b"hello\n\nworld"), b"hello\nworld");
    }

    #[test]
    fn test_strip_newlines_removes_whitespace_only_lines() {
        assert_eq!(strip_newlines(b"hello\n   \nworld"), b"hello\nworld");
    }

    #[test]
    fn test_strip_newlines_complex() {
        let input = b"\n\nhello\n  \n\nworld\nfoo\n\n";
        assert_eq!(strip_newlines(input), b"hello\nworld\nfoo");
    }

    #[test]
    fn test_shell_state_name() {
        assert_eq!(ShellState::NotStarted.name(), "not_started");
        assert_eq!(ShellState::Idle.name(), "idle");
        assert_eq!(ShellState::Running.name(), "running");
        assert_eq!(ShellState::Terminated.name(), "terminated");
        assert_eq!(ShellState::Dead.name(), "dead");
    }

    // --- Helper to create a test shell backed by a pipe ---

    fn make_test_shell() -> (RemoteShell, std::os::fd::OwnedFd) {
        let (read_fd, write_fd) = nix::unistd::pipe().unwrap();
        let shell = RemoteShell::new(
            ShellId(0),
            "testhost".into(),
            "22".into(),
            "testhost".into(),
            1,
            write_fd,
            false,
            None,
            None,
            0,
            false,
        );
        (shell, read_fd)
    }

    // --- print_unfinished_line tests ---

    #[tokio::test]
    async fn test_print_unfinished_line_flushes_running_buffer() {
        let (mut shell, _read_fd) = make_test_shell();
        let mut console = Console::new(false, None).await;

        shell.state = ShellState::Running;
        shell.read_buffer = b"Do you want to continue? [Y/n] ".to_vec();

        shell.print_unfinished_line(&mut console, 8).await;

        assert!(shell.read_buffer.is_empty());
    }

    #[tokio::test]
    async fn test_print_unfinished_line_noop_when_idle() {
        let (mut shell, _read_fd) = make_test_shell();
        let mut console = Console::new(false, None).await;

        shell.state = ShellState::Idle;
        shell.read_buffer = b"some data".to_vec();

        shell.print_unfinished_line(&mut console, 8).await;

        assert_eq!(shell.read_buffer, b"some data");
    }

    #[tokio::test]
    async fn test_print_unfinished_line_noop_when_buffer_empty() {
        let (mut shell, _read_fd) = make_test_shell();
        let mut console = Console::new(false, None).await;

        shell.state = ShellState::Running;

        shell.print_unfinished_line(&mut console, 8).await;

        assert!(shell.read_buffer.is_empty());
    }

    // --- dispatch_command tests ---

    #[tokio::test]
    async fn test_dispatch_command_idle_to_running() {
        let (mut shell, read_fd) = make_test_shell();

        shell.state = ShellState::Idle;
        shell.dispatch_command(b"sleep 5\n").await;

        assert_eq!(shell.state, ShellState::Running);

        let mut buf = [0u8; 64];
        let n = nix::unistd::read(read_fd.as_fd(), &mut buf).unwrap();
        assert_eq!(&buf[..n], b"sleep 5\n");
    }

    #[tokio::test]
    async fn test_dispatch_command_forwards_while_running() {
        let (mut shell, read_fd) = make_test_shell();

        shell.state = ShellState::Running;
        shell.dispatch_command(b"y\n").await;

        // State stays Running
        assert_eq!(shell.state, ShellState::Running);

        let mut buf = [0u8; 64];
        let n = nix::unistd::read(read_fd.as_fd(), &mut buf).unwrap();
        assert_eq!(&buf[..n], b"y\n");
    }

    #[tokio::test]
    async fn test_dispatch_command_ignored_when_dead() {
        let (mut shell, read_fd) = make_test_shell();

        shell.state = ShellState::Dead;
        shell.dispatch_command(b"y\n").await;

        assert_eq!(shell.state, ShellState::Dead);

        // Set non-blocking to confirm nothing was written
        let flags =
            nix::fcntl::fcntl(read_fd.as_fd(), nix::fcntl::FcntlArg::F_GETFL).unwrap();
        let mut oflags = nix::fcntl::OFlag::from_bits_truncate(flags);
        oflags.insert(nix::fcntl::OFlag::O_NONBLOCK);
        nix::fcntl::fcntl(read_fd.as_fd(), nix::fcntl::FcntlArg::F_SETFL(oflags)).unwrap();

        let mut buf = [0u8; 64];
        assert!(nix::unistd::read(read_fd.as_fd(), &mut buf).is_err());
    }

    // --- write_to_pty tests (used for Ctrl-C forwarding) ---

    #[test]
    fn test_write_to_pty_sends_ctrl_c() {
        let (read_fd, write_fd) = nix::unistd::pipe().unwrap();
        let shell = RemoteShell::new(
            ShellId(0), "h".into(), "22".into(), "h".into(),
            1, write_fd, false, None, None, 0, false,
        );

        shell.write_to_pty(b"\x03");

        let mut buf = [0u8; 64];
        let n = nix::unistd::read(read_fd.as_fd(), &mut buf).unwrap();
        assert_eq!(&buf[..n], b"\x03");
    }

    #[test]
    fn test_dispatch_write_disabled_shell() {
        let (read_fd, write_fd) = nix::unistd::pipe().unwrap();
        let mut shell = RemoteShell::new(
            ShellId(0), "h".into(), "22".into(), "h".into(),
            1, write_fd, false, None, None, 0, false,
        );

        shell.state = ShellState::Running;
        shell.enabled = false;
        assert!(!shell.dispatch_write(b"test"));

        let flags =
            nix::fcntl::fcntl(read_fd.as_fd(), nix::fcntl::FcntlArg::F_GETFL).unwrap();
        let mut oflags = nix::fcntl::OFlag::from_bits_truncate(flags);
        oflags.insert(nix::fcntl::OFlag::O_NONBLOCK);
        nix::fcntl::fcntl(read_fd.as_fd(), nix::fcntl::FcntlArg::F_SETFL(oflags)).unwrap();

        let mut buf = [0u8; 64];
        assert!(nix::unistd::read(read_fd.as_fd(), &mut buf).is_err());
    }
}
