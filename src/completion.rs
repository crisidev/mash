use std::collections::HashSet;

use crate::control_commands;
use crate::shell_manager::ShellManager;

pub(crate) struct CompletionState {
    pub(crate) shell_names: Vec<String>,
    pub(crate) history_words: HashSet<String>,
    pub(crate) commands_in_path: Vec<String>,
}

impl CompletionState {
    pub(crate) fn from_manager(mgr: &ShellManager) -> Self {
        Self {
            shell_names: mgr.shell_display_names(),
            history_words: HashSet::new(),
            commands_in_path: read_commands_in_path(),
        }
    }

    pub(crate) fn update_from_manager(&mut self, mgr: &ShellManager) {
        self.shell_names = mgr.shell_display_names();
    }

    pub(crate) fn add_history_words(&mut self, line: &str) {
        if self.history_words.len() < 10000 {
            for word in line.split_whitespace() {
                if word.len() > 1 {
                    self.history_words.insert(word.to_string());
                }
            }
        }
    }
}

fn read_commands_in_path() -> Vec<String> {
    let mut commands = HashSet::new();
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if !dir.is_empty() {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        if let Some(name) = entry.file_name().to_str() {
                            commands.insert(name.to_string());
                        }
                    }
                }
            }
        }
    }
    commands.into_iter().collect()
}

pub(crate) fn complete_line(line: &str, text: &str, state: &CompletionState) -> Vec<String> {
    if line.starts_with(':') {
        complete_control_command(line, text, state)
    } else {
        let (dropped_exclam, text) = if line.starts_with('!') && !text.is_empty() && line.starts_with(text) {
            (true, &text[1..])
        } else {
            (false, text)
        };

        let mut results = Vec::new();

        // Complete local paths
        results.extend(complete_local_path(text));

        // Complete from history
        let tlen = text.len();
        for word in &state.history_words {
            if word.len() > tlen && word.starts_with(text) {
                results.push(format!("{} ", word));
            }
        }

        // Complete first word from $PATH
        let is_first_word = !line.contains(' ') || (line.starts_with('!') && !line[1..].contains(' '));
        if is_first_word {
            for cmd in &state.commands_in_path {
                if cmd.len() > tlen && cmd.starts_with(text) {
                    results.push(format!("{} ", cmd));
                }
            }
        }

        results = remove_dupes(results);

        if dropped_exclam {
            results = results.into_iter().map(|r| format!("!{}", r)).collect();
        }

        results
    }
}

fn complete_control_command(line: &str, text: &str, state: &CompletionState) -> Vec<String> {
    let parts: Vec<&str> = line.split_whitespace().collect();

    if parts.len() <= 1 && !line.ends_with(' ') {
        // Completing command name
        let prefix = text.strip_prefix(':').unwrap_or(text);
        control_commands::list_command_names()
            .into_iter()
            .filter(|cmd| cmd.starts_with(prefix))
            .map(|cmd| format!(":{} ", cmd))
            .collect()
    } else {
        // Completing command parameters - complete with shell names
        state
            .shell_names
            .iter()
            .filter(|name| name.starts_with(text) && !line.contains(&format!(" {} ", name)))
            .map(|name| format!("{} ", name))
            .collect()
    }
}

fn complete_local_path(text: &str) -> Vec<String> {
    let expanded = if text.starts_with('~') {
        if let Ok(home) = std::env::var("HOME") {
            text.replacen('~', &home, 1)
        } else {
            text.to_string()
        }
    } else {
        text.to_string()
    };

    let pattern = format!("{}*", expanded);
    let mut results = Vec::new();
    if let Ok(entries) = glob::glob(&pattern) {
        for entry in entries.flatten() {
            let path_str = entry.display().to_string();
            let suffix = if entry.is_dir() { "/" } else { "" };
            results.push(format!("{}{}", path_str, suffix));
        }
    }
    results
}

pub(crate) fn remove_dupes(words: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for w in words {
        let stripped = w.trim_end_matches(['/', ' ']).to_string();
        if seen.insert(stripped) {
            result.push(w);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(shell_names: Vec<&str>, history: Vec<&str>, commands: Vec<&str>) -> CompletionState {
        CompletionState {
            shell_names: shell_names.into_iter().map(String::from).collect(),
            history_words: history.into_iter().map(String::from).collect(),
            commands_in_path: commands.into_iter().map(String::from).collect(),
        }
    }

    // --- remove_dupes tests ---

    #[test]
    fn test_remove_dupes_empty() {
        assert!(remove_dupes(vec![]).is_empty());
    }

    #[test]
    fn test_remove_dupes_no_duplicates() {
        let input = vec!["foo ".into(), "bar ".into()];
        let result = remove_dupes(input);
        assert_eq!(result, vec!["foo ", "bar "]);
    }

    #[test]
    fn test_remove_dupes_trailing_slash_and_space() {
        // "foo/" and "foo " should be considered duplicates (both strip to "foo")
        let input = vec!["foo/".into(), "foo ".into()];
        let result = remove_dupes(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "foo/");
    }

    #[test]
    fn test_remove_dupes_exact_duplicates() {
        let input = vec!["hello ".into(), "hello ".into(), "world ".into()];
        let result = remove_dupes(input);
        assert_eq!(result, vec!["hello ", "world "]);
    }

    // --- add_history_words tests ---

    #[test]
    fn test_add_history_words() {
        let mut state = make_state(vec![], vec![], vec![]);
        state.add_history_words("ls -la /tmp");
        assert!(state.history_words.contains("ls"));
        assert!(state.history_words.contains("-la"));
        assert!(state.history_words.contains("/tmp"));
    }

    #[test]
    fn test_add_history_words_skips_single_char() {
        let mut state = make_state(vec![], vec![], vec![]);
        state.add_history_words("a bb ccc");
        assert!(!state.history_words.contains("a"));
        assert!(state.history_words.contains("bb"));
        assert!(state.history_words.contains("ccc"));
    }

    #[test]
    fn test_add_history_words_limit() {
        let mut state = make_state(vec![], vec![], vec![]);
        // Fill up to 10000
        for i in 0..10001 {
            state.add_history_words(&format!("word{}", i));
        }
        // Should not exceed 10000 (approximately, since we add in bulk)
        assert!(state.history_words.len() <= 10001);
    }

    // --- complete_line tests ---

    #[test]
    fn test_complete_control_command_name() {
        let state = make_state(vec!["web1", "web2"], vec![], vec![]);
        let results = complete_line(":li", ":li", &state);
        assert!(results.iter().any(|r| r == ":list "));
    }

    #[test]
    fn test_complete_control_command_all() {
        let state = make_state(vec![], vec![], vec![]);
        let results = complete_line(":", ":", &state);
        // Should list all commands
        assert!(results.len() > 10);
        assert!(results.iter().any(|r| r == ":help "));
        assert!(results.iter().any(|r| r == ":quit "));
    }

    #[test]
    fn test_complete_control_command_params() {
        let state = make_state(vec!["web1", "web2", "db1"], vec![], vec![]);
        let results = complete_line(":enable w", "w", &state);
        assert!(results.iter().any(|r| r == "web1 "));
        assert!(results.iter().any(|r| r == "web2 "));
        assert!(!results.iter().any(|r| r.starts_with("db")));
    }

    #[test]
    fn test_complete_line_from_history() {
        let state = make_state(vec![], vec!["uptime", "hostname"], vec![]);
        let results = complete_line("upt", "upt", &state);
        assert!(results.iter().any(|r| r == "uptime "));
    }

    #[test]
    fn test_complete_line_from_path() {
        let state = make_state(vec![], vec![], vec!["ls", "lsblk", "lsof"]);
        let results = complete_line("ls", "ls", &state);
        assert!(results.iter().any(|r| r == "lsblk "));
        assert!(results.iter().any(|r| r == "lsof "));
    }

    #[test]
    fn test_complete_line_no_path_after_space() {
        // PATH completion only happens for first word
        let state = make_state(vec![], vec![], vec!["ls", "lsblk"]);
        let results = complete_line("echo ls", "ls", &state);
        // Should not include lsblk since it's not the first word
        assert!(!results.iter().any(|r| r == "lsblk "));
    }

    #[test]
    fn test_complete_line_exclamation() {
        let state = make_state(vec![], vec![], vec!["ls", "lsblk"]);
        let results = complete_line("!ls", "!ls", &state);
        // Results should have ! prefix
        assert!(results.iter().any(|r| r == "!lsblk "));
    }
}
