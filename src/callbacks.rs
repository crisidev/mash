use std::collections::HashMap;

use rand::RngExt;

fn random_string(length: usize) -> String {
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut rng = rand::rng();
    (0..length)
        .map(|_| CHARS[rng.random_range(0..CHARS.len())] as char)
        .collect()
}

#[derive(Debug, Clone)]
pub(crate) enum CallbackAction {
    SeenPrompt,
    Rename { new_name: Vec<u8> },
    None,
}

struct CallbackEntry {
    action: CallbackAction,
    repeat: bool,
}

pub(crate) struct CallbackRegistry {
    common_prefix: Vec<u8>,
    callbacks: HashMap<Vec<u8>, CallbackEntry>,
    nr_generated: usize,
}

impl CallbackRegistry {
    pub(crate) fn new() -> Self {
        let prefix = format!("mash-{}:", random_string(5)).into_bytes();
        Self {
            common_prefix: prefix,
            callbacks: HashMap::new(),
            nr_generated: 0,
        }
    }

    /// Register a callback and return (trigger_part1, trigger_part2) for split-echo.
    pub(crate) fn add(&mut self, name: &[u8], action: CallbackAction, repeat: bool) -> (Vec<u8>, Vec<u8>) {
        let name_safe: Vec<u8> = name.iter().map(|&b| if b == b'/' { b'_' } else { b }).collect();
        let nr = self.nr_generated;
        self.nr_generated += 1;

        let trigger = format!(
            "{}{}:{}:{}/",
            String::from_utf8_lossy(&self.common_prefix),
            String::from_utf8_lossy(&name_safe),
            random_string(5),
            nr,
        )
        .into_bytes();

        self.callbacks.insert(trigger.clone(), CallbackEntry { action, repeat });

        let split = self.common_prefix.len() / 2;
        (trigger[..split].to_vec(), trigger[split..].to_vec())
    }

    /// Check if the common prefix appears anywhere in data (fast check).
    pub(crate) fn any_in(&self, data: &[u8]) -> bool {
        data.windows(self.common_prefix.len())
            .any(|w| w == self.common_prefix.as_slice())
    }

    /// Return the common prefix (for testing).
    #[cfg(test)]
    pub fn common_prefix(&self) -> &[u8] {
        &self.common_prefix
    }

    /// Process a line looking for callback triggers.
    /// Returns Some(action) if a trigger was found.
    pub(crate) fn process(&mut self, line: &[u8]) -> Option<CallbackAction> {
        let start = line
            .windows(self.common_prefix.len())
            .position(|w| w == self.common_prefix.as_slice())?;

        let end = line[start..].iter().position(|&b| b == b'/')?;
        let end = start + end + 1;

        let trigger = line[start..end].to_vec();
        let remainder = line[end..].to_vec();

        let entry = self.callbacks.get(&trigger)?;
        let mut action = entry.action.clone();
        let repeat = entry.repeat;

        // For rename, attach the remainder as the new name
        if let CallbackAction::Rename { ref mut new_name } = action {
            let trimmed: Vec<u8> = remainder.iter().copied().filter(|&b| b != b'\n' && b != b' ').collect();
            *new_name = trimmed;
        }

        if !repeat {
            self.callbacks.remove(&trigger);
        }

        Some(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_string_length() {
        assert_eq!(random_string(0).len(), 0);
        assert_eq!(random_string(10).len(), 10);
        assert_eq!(random_string(100).len(), 100);
    }

    #[test]
    fn test_random_string_alphanumeric() {
        let s = random_string(50);
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn test_random_string_unique() {
        let a = random_string(20);
        let b = random_string(20);
        assert_ne!(a, b);
    }

    #[test]
    fn test_registry_prefix_format() {
        let reg = CallbackRegistry::new();
        let prefix = String::from_utf8_lossy(reg.common_prefix());
        assert!(prefix.starts_with("mash-"));
        assert!(prefix.ends_with(':'));
        // "mash-XXXXX:" = 11 chars
        assert_eq!(prefix.len(), 11);
    }

    #[test]
    fn test_add_returns_split_trigger() {
        let mut reg = CallbackRegistry::new();
        let (p1, p2) = reg.add(b"test", CallbackAction::SeenPrompt, false);
        // The two parts together should form the full trigger
        let full: Vec<u8> = [p1.as_slice(), p2.as_slice()].concat();
        let full_str = String::from_utf8_lossy(&full);
        assert!(full_str.starts_with("mash-"));
        assert!(full_str.ends_with('/'));
        assert!(full_str.contains(":test:"));
    }

    #[test]
    fn test_add_replaces_slashes_in_name() {
        let mut reg = CallbackRegistry::new();
        let (p1, p2) = reg.add(b"a/b/c", CallbackAction::SeenPrompt, false);
        let full: Vec<u8> = [p1.as_slice(), p2.as_slice()].concat();
        let full_str = String::from_utf8_lossy(&full);
        assert!(full_str.contains(":a_b_c:"));
    }

    #[test]
    fn test_add_increments_nr() {
        let mut reg = CallbackRegistry::new();
        let (p1a, p2a) = reg.add(b"x", CallbackAction::SeenPrompt, false);
        let (p1b, p2b) = reg.add(b"x", CallbackAction::SeenPrompt, false);
        let a: Vec<u8> = [p1a.as_slice(), p2a.as_slice()].concat();
        let b: Vec<u8> = [p1b.as_slice(), p2b.as_slice()].concat();
        assert_ne!(a, b);
        // First ends with :0/, second with :1/
        let a_str = String::from_utf8_lossy(&a);
        let b_str = String::from_utf8_lossy(&b);
        assert!(a_str.contains(":0/"));
        assert!(b_str.contains(":1/"));
    }

    #[test]
    fn test_any_in_finds_prefix() {
        let reg = CallbackRegistry::new();
        let prefix = reg.common_prefix().to_vec();
        let mut data = b"some data ".to_vec();
        data.extend_from_slice(&prefix);
        data.extend_from_slice(b" more data");
        assert!(reg.any_in(&data));
    }

    #[test]
    fn test_any_in_no_match() {
        let reg = CallbackRegistry::new();
        assert!(!reg.any_in(b"no callback here"));
        assert!(!reg.any_in(b""));
    }

    #[test]
    fn test_process_seen_prompt() {
        let mut reg = CallbackRegistry::new();
        let (p1, p2) = reg.add(b"prompt", CallbackAction::SeenPrompt, true);
        let mut line = Vec::new();
        line.extend_from_slice(&p1);
        line.extend_from_slice(&p2);
        line.push(b'\n');

        let action = reg.process(&line);
        assert!(matches!(action, Some(CallbackAction::SeenPrompt)));
    }

    #[test]
    fn test_process_repeat_keeps_callback() {
        let mut reg = CallbackRegistry::new();
        let (p1, p2) = reg.add(b"prompt", CallbackAction::SeenPrompt, true);
        let mut line = Vec::new();
        line.extend_from_slice(&p1);
        line.extend_from_slice(&p2);
        line.push(b'\n');

        // Process twice — should work both times since repeat=true
        assert!(reg.process(&line).is_some());
        assert!(reg.process(&line).is_some());
    }

    #[test]
    fn test_process_no_repeat_removes_callback() {
        let mut reg = CallbackRegistry::new();
        let (p1, p2) = reg.add(b"once", CallbackAction::SeenPrompt, false);
        let mut line = Vec::new();
        line.extend_from_slice(&p1);
        line.extend_from_slice(&p2);
        line.push(b'\n');

        assert!(reg.process(&line).is_some());
        assert!(reg.process(&line).is_none());
    }

    #[test]
    fn test_process_rename_captures_remainder() {
        let mut reg = CallbackRegistry::new();
        let (p1, p2) = reg.add(b"rename", CallbackAction::Rename { new_name: Vec::new() }, false);
        let mut line = Vec::new();
        line.extend_from_slice(&p1);
        line.extend_from_slice(&p2);
        line.extend_from_slice(b"newhost\n");

        let action = reg.process(&line);
        match action {
            Some(CallbackAction::Rename { new_name }) => {
                assert_eq!(new_name, b"newhost");
            }
            _ => panic!("Expected Rename action"),
        }
    }

    #[test]
    fn test_process_rename_strips_whitespace() {
        let mut reg = CallbackRegistry::new();
        let (p1, p2) = reg.add(b"rename", CallbackAction::Rename { new_name: Vec::new() }, false);
        let mut line = Vec::new();
        line.extend_from_slice(&p1);
        line.extend_from_slice(&p2);
        line.extend_from_slice(b" my host \n");

        let action = reg.process(&line);
        match action {
            Some(CallbackAction::Rename { new_name }) => {
                assert_eq!(new_name, b"myhost");
            }
            _ => panic!("Expected Rename action"),
        }
    }

    #[test]
    fn test_process_no_trigger() {
        let mut reg = CallbackRegistry::new();
        reg.add(b"test", CallbackAction::SeenPrompt, false);
        assert!(reg.process(b"random data without trigger\n").is_none());
    }
}
