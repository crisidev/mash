use std::collections::HashMap;

pub(crate) struct DisplayNameRegistry {
    prefixes: HashMap<String, Vec<bool>>,
    nr_enabled_by_length: HashMap<usize, usize>,
    pub(crate) max_display_name_length: usize,
}

impl DisplayNameRegistry {
    pub(crate) fn new() -> Self {
        Self {
            prefixes: HashMap::new(),
            nr_enabled_by_length: HashMap::new(),
            max_display_name_length: 0,
        }
    }

    fn acquire_prefix_index(&mut self, prefix: &str) -> usize {
        let slots = self.prefixes.entry(prefix.to_string()).or_default();
        for (idx, in_use) in slots.iter_mut().enumerate() {
            if !*in_use {
                *in_use = true;
                return idx;
            }
        }
        slots.push(true);
        slots.len() - 1
    }

    fn release_prefix_index(&mut self, display_name: &str) {
        let (prefix, suffix) = if let Some((p, s)) = display_name.split_once('#') {
            (p.to_string(), s.parse::<usize>().unwrap_or(0))
        } else {
            (display_name.to_string(), 0)
        };

        let slots = match self.prefixes.get_mut(&prefix) {
            Some(s) => s,
            None => return,
        };

        if suffix < slots.len().saturating_sub(1) {
            slots[suffix] = false;
            return;
        }

        if suffix < slots.len() {
            slots.remove(suffix);
        }

        // Remove trailing holes
        while let Some(false) = slots.last() {
            slots.pop();
        }

        if slots.is_empty() {
            self.prefixes.remove(&prefix);
        }
    }

    fn make_unique_name(&mut self, prefix: &str) -> String {
        let suffix = self.acquire_prefix_index(prefix);
        if suffix == 0 {
            prefix.to_string()
        } else {
            format!("{}#{}", prefix, suffix)
        }
    }

    fn update_max_length(&mut self) {
        self.max_display_name_length = self.nr_enabled_by_length.keys().copied().max().unwrap_or(0);
    }

    pub(crate) fn change(&mut self, prev_display_name: Option<&str>, new_prefix: Option<&str>) -> Option<String> {
        if let Some(new_p) = new_prefix {
            if new_p.contains('#') {
                panic!("Names cannot contain #");
            }
        }

        if let Some(prev) = prev_display_name {
            if new_prefix.is_some() {
                self.set_enabled(prev, false);
            }
            self.release_prefix_index(prev);
            new_prefix?;
        }

        let name = self.make_unique_name(new_prefix.unwrap());
        self.set_enabled(&name, true);
        Some(name)
    }

    pub(crate) fn set_enabled(&mut self, display_name: &str, enabled: bool) {
        let length = display_name.len();
        if enabled {
            *self.nr_enabled_by_length.entry(length).or_insert(0) += 1;
        } else {
            let entry = self.nr_enabled_by_length.entry(length).or_insert(0);
            *entry = entry.saturating_sub(1);
            if *entry == 0 {
                self.nr_enabled_by_length.remove(&length);
            }
        }
        self.update_max_length();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unique_names() {
        let mut reg = DisplayNameRegistry::new();
        let n1 = reg.change(None, Some("host")).unwrap();
        assert_eq!(n1, "host");
        let n2 = reg.change(None, Some("host")).unwrap();
        assert_eq!(n2, "host#1");
        let n3 = reg.change(None, Some("host")).unwrap();
        assert_eq!(n3, "host#2");
    }

    #[test]
    fn test_release_and_reuse() {
        let mut reg = DisplayNameRegistry::new();
        let n1 = reg.change(None, Some("host")).unwrap();
        let _n2 = reg.change(None, Some("host")).unwrap();
        // Release first one
        reg.change(Some(&n1), None);
        // Should reuse index 0
        let n3 = reg.change(None, Some("host")).unwrap();
        assert_eq!(n3, "host");
    }

    #[test]
    fn test_max_length() {
        let mut reg = DisplayNameRegistry::new();
        let _n1 = reg.change(None, Some("short"));
        assert_eq!(reg.max_display_name_length, 5);
        let _n2 = reg.change(None, Some("longername"));
        assert_eq!(reg.max_display_name_length, 10);
    }

    #[test]
    fn test_max_length_after_removal() {
        let mut reg = DisplayNameRegistry::new();
        let n1 = reg.change(None, Some("short")).unwrap();
        let n2 = reg.change(None, Some("longername")).unwrap();
        assert_eq!(reg.max_display_name_length, 10);
        // Disable then remove the longer name (mirrors real usage)
        reg.set_enabled(&n2, false);
        reg.change(Some(&n2), None);
        assert_eq!(reg.max_display_name_length, 5);
        // Disable then remove the shorter name too
        reg.set_enabled(&n1, false);
        reg.change(Some(&n1), None);
        assert_eq!(reg.max_display_name_length, 0);
    }

    #[test]
    fn test_rename() {
        let mut reg = DisplayNameRegistry::new();
        let n1 = reg.change(None, Some("oldname")).unwrap();
        assert_eq!(n1, "oldname");
        let n2 = reg.change(Some(&n1), Some("newname")).unwrap();
        assert_eq!(n2, "newname");
        // oldname should be released — adding it again should get index 0
        let n3 = reg.change(None, Some("oldname")).unwrap();
        assert_eq!(n3, "oldname");
    }

    #[test]
    fn test_set_enabled_tracking() {
        let mut reg = DisplayNameRegistry::new();
        let n1 = reg.change(None, Some("host")).unwrap();
        assert_eq!(reg.max_display_name_length, 4);
        reg.set_enabled(&n1, false);
        assert_eq!(reg.max_display_name_length, 0);
        reg.set_enabled(&n1, true);
        assert_eq!(reg.max_display_name_length, 4);
    }

    #[test]
    fn test_many_duplicates() {
        let mut reg = DisplayNameRegistry::new();
        let n1 = reg.change(None, Some("srv")).unwrap();
        let n2 = reg.change(None, Some("srv")).unwrap();
        let n3 = reg.change(None, Some("srv")).unwrap();
        assert_eq!(n1, "srv");
        assert_eq!(n2, "srv#1");
        assert_eq!(n3, "srv#2");
        // Release middle one
        reg.change(Some(&n2), None);
        // Next should reuse #1
        let n4 = reg.change(None, Some("srv")).unwrap();
        assert_eq!(n4, "srv#1");
    }

    #[test]
    #[should_panic(expected = "Names cannot contain #")]
    fn test_hash_in_name_panics() {
        let mut reg = DisplayNameRegistry::new();
        reg.change(None, Some("bad#name"));
    }
}
