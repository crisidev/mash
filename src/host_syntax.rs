use std::sync::LazyLock;

use regex::Regex;

static SYNTAX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<([0-9,\-]+)>").unwrap());
static INTERVAL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9]+)(-[0-9]+)?$").unwrap());

pub(crate) fn split_port(hostname: &str) -> (String, String) {
    match hostname.split_once(':') {
        Some((host, port)) => (host.to_string(), port.to_string()),
        None => (hostname.to_string(), "22".to_string()),
    }
}

fn iter_numbers(start: &str, end: &str) -> Vec<String> {
    let s: i64 = start.parse().unwrap_or(0);
    let e: i64 = end.parse().unwrap_or(0);
    let zero_pad = (start.len() > 1 && start.starts_with('0')) || (end.len() > 1 && end.starts_with('0'));
    let width = start.len().max(end.len());
    let increment: i64 = if s <= e { 1 } else { -1 };

    let mut results = Vec::new();
    let mut i = s;
    loop {
        let formatted = if zero_pad {
            format!("{:0>width$}", i, width = width)
        } else {
            i.to_string()
        };
        results.push(formatted);
        if i == e {
            break;
        }
        i += increment;
    }
    results
}

pub(crate) fn expand_syntax(input: &str) -> Vec<String> {
    if let Some(m) = SYNTAX_RE.find(input) {
        let prefix = &input[..m.start()];
        let suffix = &input[m.end()..];
        let inner = &input[m.start() + 1..m.end() - 1];
        let mut results = Vec::new();

        for interval in inner.split(',') {
            if let Some(caps) = INTERVAL_RE.captures(interval) {
                let start = caps.get(1).unwrap().as_str();
                let end = caps
                    .get(2)
                    .map(|m| &m.as_str()[1..]) // strip leading '-'
                    .unwrap_or(start);
                for num_str in iter_numbers(start, end) {
                    let combined = format!("{}{}{}", prefix, num_str, suffix);
                    results.extend(expand_syntax(&combined));
                }
            }
        }
        results
    } else {
        vec![input.to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_range() {
        let result = expand_syntax("host<1-3>");
        assert_eq!(result, vec!["host1", "host2", "host3"]);
    }

    #[test]
    fn test_reverse_range() {
        let result = expand_syntax("host<3-1>");
        assert_eq!(result, vec!["host3", "host2", "host1"]);
    }

    #[test]
    fn test_zero_padded() {
        let result = expand_syntax("host<01-03>");
        assert_eq!(result, vec!["host01", "host02", "host03"]);
    }

    #[test]
    fn test_comma_separated() {
        let result = expand_syntax("host<1,3-5>");
        assert_eq!(result, vec!["host1", "host3", "host4", "host5"]);
    }

    #[test]
    fn test_single_number() {
        let result = expand_syntax("host<1>");
        assert_eq!(result, vec!["host1"]);
    }

    #[test]
    fn test_no_expansion() {
        let result = expand_syntax("hostname");
        assert_eq!(result, vec!["hostname"]);
    }

    #[test]
    fn test_split_port() {
        assert_eq!(split_port("host:2222"), ("host".into(), "2222".into()));
        assert_eq!(split_port("host"), ("host".into(), "22".into()));
    }

    #[test]
    fn test_nested_expansion() {
        // Double expansion: prefix<1-2><a-b> should not work (no alpha ranges)
        // But prefix<1-2> with suffix<3-4> should work
        let result = expand_syntax("h<1-2>s<3-4>");
        assert_eq!(result, vec!["h1s3", "h1s4", "h2s3", "h2s4"]);
    }

    #[test]
    fn test_prefix_and_suffix() {
        let result = expand_syntax("pre<1-3>.example.com");
        assert_eq!(
            result,
            vec!["pre1.example.com", "pre2.example.com", "pre3.example.com",]
        );
    }

    #[test]
    fn test_empty_input() {
        let result = expand_syntax("");
        assert_eq!(result, vec![""]);
    }

    #[test]
    fn test_large_zero_padded_range() {
        let result = expand_syntax("node<001-003>");
        assert_eq!(result, vec!["node001", "node002", "node003"]);
    }

    #[test]
    fn test_comma_single_values() {
        let result = expand_syntax("host<1,5,9>");
        assert_eq!(result, vec!["host1", "host5", "host9"]);
    }
}
