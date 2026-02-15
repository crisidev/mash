use std::collections::BTreeMap;
use std::os::fd::OwnedFd;

use crate::display_names::DisplayNameRegistry;
use crate::shell::{RemoteShell, ShellId, ShellState};

pub(crate) struct ShellManager {
    shells: BTreeMap<ShellId, RemoteShell>,
    next_id: usize,
    color_rotation: usize,
    use_color: bool,
}

impl ShellManager {
    pub(crate) fn new(use_color: bool) -> Self {
        Self {
            shells: BTreeMap::new(),
            next_id: 0,
            color_rotation: 0,
            use_color,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add_shell(
        &mut self,
        hostname: String,
        port: String,
        pid: i32,
        master_fd: OwnedFd,
        debug: bool,
        command: Option<String>,
        password: Option<String>,
        display_names: &mut DisplayNameRegistry,
    ) -> ShellId {
        let id = ShellId(self.next_id);
        self.next_id += 1;

        let display_name = display_names
            .change(None, Some(&hostname))
            .unwrap_or_else(|| hostname.clone());

        let color_idx = self.color_rotation;
        self.color_rotation += 1;

        let shell = RemoteShell::new(
            id,
            hostname,
            port,
            display_name,
            pid,
            master_fd,
            debug,
            command,
            password,
            color_idx,
            self.use_color,
        );

        self.shells.insert(id, shell);
        id
    }

    pub(crate) fn get_shell(&self, id: ShellId) -> Option<&RemoteShell> {
        self.shells.get(&id)
    }

    pub(crate) fn get_shell_mut(&mut self, id: ShellId) -> Option<&mut RemoteShell> {
        self.shells.get_mut(&id)
    }

    pub(crate) fn remove_shell(&mut self, id: ShellId) {
        self.shells.remove(&id);
    }

    /// All shells sorted by display name
    pub(crate) fn all_shells(&self) -> Vec<&RemoteShell> {
        let mut shells: Vec<&RemoteShell> = self.shells.values().collect();
        shells.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        shells
    }

    /// All shells sorted by display name (mutable)
    pub(crate) fn all_shells_mut(&mut self) -> Vec<&mut RemoteShell> {
        let mut shells: Vec<&mut RemoteShell> = self.shells.values_mut().collect();
        shells.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        shells
    }

    /// Returns (awaiting_count, total_enabled_count)
    pub(crate) fn count_awaited_processes(&self) -> (usize, usize) {
        let mut awaited = 0;
        let mut total = 0;
        for shell in self.shells.values() {
            if shell.enabled {
                total += 1;
                if shell.state != ShellState::Idle {
                    awaited += 1;
                }
            }
        }
        (awaited, total)
    }

    /// Returns counts by state: (idle, running, not_started, dead, disabled)
    pub(crate) fn count_by_state(&self) -> (usize, usize, usize, usize, usize) {
        let (mut idle, mut running, mut not_started, mut dead, mut disabled) = (0, 0, 0, 0, 0);
        for shell in self.shells.values() {
            if !shell.enabled {
                disabled += 1;
            } else {
                match shell.state {
                    ShellState::Idle => idle += 1,
                    ShellState::Running => running += 1,
                    ShellState::NotStarted => not_started += 1,
                    ShellState::Terminated | ShellState::Dead => dead += 1,
                }
            }
        }
        (idle, running, not_started, dead, disabled)
    }

    pub(crate) fn all_terminated(&self) -> bool {
        if self.shells.is_empty() {
            return false;
        }
        self.shells
            .values()
            .all(|s| s.state == ShellState::Terminated || s.state == ShellState::Dead)
    }

    pub(crate) fn format_info(info_list: &[Vec<Vec<u8>>]) -> Vec<Vec<u8>> {
        if info_list.is_empty() {
            return Vec::new();
        }

        let nr_columns = info_list[0].len();
        let mut max_lengths = vec![0usize; nr_columns];
        for info in info_list {
            for (i, col) in info.iter().enumerate() {
                max_lengths[i] = max_lengths[i].max(col.len());
            }
        }

        let mut result = Vec::new();
        for info in info_list {
            let mut line = Vec::new();
            for (i, col) in info.iter().enumerate() {
                if i > 0 {
                    line.push(b' ');
                }
                line.extend_from_slice(col);
                // Don't pad the last column
                if i < nr_columns - 1 {
                    let padding = max_lengths[i].saturating_sub(col.len());
                    line.extend(std::iter::repeat_n(b' ', padding));
                }
            }
            line.push(b'\n');
            result.push(line);
        }
        result
    }

    pub(crate) fn shell_ids(&self) -> Vec<ShellId> {
        self.shells.keys().copied().collect()
    }

    pub(crate) fn shell_display_names(&self) -> Vec<String> {
        self.shells.values().map(|s| s.display_name.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- format_info tests ---

    #[test]
    fn test_format_info_empty() {
        let result = ShellManager::format_info(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_info_single_row() {
        let info = vec![vec![b"host1".to_vec(), b"enabled".to_vec(), b"idle:".to_vec()]];
        let result = ShellManager::format_info(&info);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], b"host1 enabled idle:\n");
    }

    #[test]
    fn test_format_info_column_alignment() {
        let info = vec![
            vec![b"h1".to_vec(), b"enabled".to_vec(), b"idle:".to_vec()],
            vec![b"longhost".to_vec(), b"disabled".to_vec(), b"dead:".to_vec()],
        ];
        let result = ShellManager::format_info(&info);
        let r0 = String::from_utf8(result[0].clone()).unwrap();
        let r1 = String::from_utf8(result[1].clone()).unwrap();
        assert!(r0.starts_with("h1"));
        assert!(r1.starts_with("longhost"));
        assert!(r0.ends_with('\n'));
        assert!(r1.ends_with('\n'));
        let enabled_pos = r0.find("enabled").unwrap();
        let disabled_pos = r1.find("disabled").unwrap();
        assert_eq!(enabled_pos, disabled_pos);
    }

    #[test]
    fn test_format_info_last_column_not_padded() {
        let info = vec![
            vec![b"a".to_vec(), b"short".to_vec()],
            vec![b"b".to_vec(), b"very long last column".to_vec()],
        ];
        let result = ShellManager::format_info(&info);
        let r0 = String::from_utf8(result[0].clone()).unwrap();
        assert!(r0.ends_with("short\n"));
    }

    // --- ShellManager basic tests ---

    #[test]
    fn test_new_manager() {
        let mgr = ShellManager::new(true);
        assert!(mgr.all_shells().is_empty());
        assert!(mgr.shell_ids().is_empty());
        assert!(mgr.shell_display_names().is_empty());
    }

    #[test]
    fn test_all_terminated_empty() {
        let mgr = ShellManager::new(true);
        assert!(!mgr.all_terminated());
    }

    #[test]
    fn test_count_awaited_empty() {
        let mgr = ShellManager::new(true);
        assert_eq!(mgr.count_awaited_processes(), (0, 0));
    }

    #[test]
    fn test_count_by_state_empty() {
        let mgr = ShellManager::new(true);
        assert_eq!(mgr.count_by_state(), (0, 0, 0, 0, 0));
    }
}
