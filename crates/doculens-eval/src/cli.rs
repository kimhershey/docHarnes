//! Hand-rolled argument parsing (the same `--name value` / `--flag` conventions as
//! `scripts/fetch-engine.mjs`); no CLI dependency is pulled into the workspace.

#[derive(Debug, Clone)]
pub struct Args {
    items: Vec<String>,
}

impl Args {
    pub fn from_env() -> Args {
        Args::new(std::env::args().skip(1).collect())
    }

    pub fn new(items: Vec<String>) -> Args {
        Args { items }
    }

    /// The first positional word (`planner`, `pipeline`, `scorecard`).
    pub fn subcommand(&self) -> Option<&str> {
        self.items.first().filter(|s| !s.starts_with("--")).map(String::as_str)
    }

    /// `--name value` (the last occurrence wins; a following `--flag` is not a value).
    pub fn opt(&self, name: &str) -> Option<&str> {
        let key = format!("--{name}");
        self.items
            .iter()
            .enumerate()
            .filter(|(_, a)| **a == key)
            .filter_map(|(i, _)| self.items.get(i + 1))
            .rfind(|v| !v.starts_with("--"))
            .map(String::as_str)
    }

    /// Every value after `--name` up to the next `--flag`, over all occurrences
    /// (`--inputs a.json b.json --inputs c.json`).
    pub fn opt_many(&self, name: &str) -> Vec<String> {
        let key = format!("--{name}");
        let mut out = Vec::new();
        let mut collecting = false;
        for a in &self.items {
            if *a == key {
                collecting = true;
                continue;
            }
            if a.starts_with("--") {
                collecting = false;
                continue;
            }
            if collecting {
                out.push(a.clone());
            }
        }
        out
    }

    pub fn flag(&self, name: &str) -> bool {
        let key = format!("--{name}");
        self.items.contains(&key)
    }

    /// Flags that are not in `known` (typos surface instead of being ignored).
    pub fn unknown_flags(&self, known: &[&str]) -> Vec<String> {
        self.items
            .iter()
            .filter(|a| a.starts_with("--"))
            .filter(|a| !known.iter().any(|k| format!("--{k}") == **a))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Args {
        Args::new(s.split_whitespace().map(String::from).collect())
    }

    #[test]
    fn parses_subcommand_options_and_flags() {
        let a = args("planner --golden g.json --baseline b.json --verbose --out");
        assert_eq!(a.subcommand(), Some("planner"));
        assert_eq!(a.opt("golden"), Some("g.json"));
        assert_eq!(a.opt("baseline"), Some("b.json"));
        assert_eq!(a.opt("out"), None);
        assert!(a.flag("verbose"));
        assert!(!a.flag("dry-run"));
        assert_eq!(a.unknown_flags(&["golden", "baseline", "verbose"]), vec!["--out".to_string()]);
    }

    #[test]
    fn collects_repeated_values() {
        let a = args("scorecard --inputs a.json b.json --out s.md --inputs c.json");
        assert_eq!(a.opt_many("inputs"), vec!["a.json", "b.json", "c.json"]);
        assert_eq!(a.opt("out"), Some("s.md"));
        assert_eq!(args("--help").subcommand(), None);
    }
}
