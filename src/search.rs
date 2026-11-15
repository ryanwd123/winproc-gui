//! The watch box's search language.
//!
//! - `excel` matches names that contain "excel" (case-insensitive).
//! - `micro edge` needs every word, in any order ("msedge" does not match, "MicrosoftEdge" does).
//! - `excel | code | python` lists processes that match any alternative.
//! - `!term` (or `-term`) leaves out names that contain the term.
//! - `=code` matches the name exactly, with or without ".exe".
//! - `^py` matches names that start with "py".
//! - `pid:1234` matches a process ID.
//! - `"two words"` keeps a space inside one term.

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Query {
    groups: Vec<Group>,
}

#[derive(Debug, Default, Clone, PartialEq)]
struct Group {
    include: Vec<Term>,
    exclude: Vec<Term>,
}

#[derive(Debug, Clone, PartialEq)]
enum Term {
    Contains(String),
    Exact(String),
    Prefix(String),
    Pid(u32),
}

impl Term {
    fn parse(t: &str) -> Option<Term> {
        if let Some(rest) = t.strip_prefix('=') {
            let rest = rest.to_lowercase();
            return (!rest.is_empty()).then_some(Term::Exact(rest));
        }
        if let Some(rest) = t.strip_prefix('^') {
            let rest = rest.to_lowercase();
            return (!rest.is_empty()).then_some(Term::Prefix(rest));
        }
        if let Some(rest) = t.strip_prefix("pid:").or_else(|| t.strip_prefix("PID:")) {
            return rest.trim().parse().ok().map(Term::Pid);
        }
        (!t.is_empty()).then(|| Term::Contains(t.to_lowercase()))
    }

    fn matches(&self, name: &str, pid: u32) -> bool {
        match self {
            Term::Contains(s) => name.contains(s.as_str()),
            Term::Prefix(s) => name.starts_with(s.as_str()),
            Term::Exact(s) => {
                name == s || name.strip_suffix(".exe").is_some_and(|n| n == s)
            }
            Term::Pid(p) => *p == pid,
        }
    }
}

/// Splits on whitespace, keeping "quoted text" together.
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

impl Query {
    pub fn parse(s: &str) -> Query {
        let mut groups = Vec::new();
        for alt in s.split('|') {
            let mut g = Group::default();
            for w in words(alt) {
                let neg = w.strip_prefix('!').or_else(|| w.strip_prefix('-').filter(|r| !r.is_empty()));
                match neg {
                    Some(rest) => g.exclude.extend(Term::parse(rest)),
                    None => g.include.extend(Term::parse(&w)),
                }
            }
            if !g.include.is_empty() || !g.exclude.is_empty() {
                groups.push(g);
            }
        }
        Query { groups }
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// `name` must already be lowercase.
    pub fn matches(&self, name: &str, pid: u32) -> bool {
        self.groups.iter().any(|g| {
            g.include.iter().all(|t| t.matches(name, pid))
                && !g.exclude.iter().any(|t| t.matches(name, pid))
        })
    }
}

/// Also marks every descendant of a marked process. `procs` holds
/// (pid, parent pid, create time) in the same order as `hit`.
pub fn add_children(procs: &[(u32, u32, u64)], hit: &mut [bool]) {
    if !hit.iter().any(|&h| h) {
        return;
    }
    let by_pid: std::collections::HashMap<u32, usize> =
        procs.iter().enumerate().map(|(i, p)| (p.0, i)).collect();
    for i in 0..procs.len() {
        if hit[i] {
            continue;
        }
        let mut cur = i;
        for _ in 0..64 {
            let (_, ppid, created) = procs[cur];
            let Some(&parent) = by_pid.get(&ppid) else { break };
            // A parent must be older than its child; otherwise the PID was reused.
            if parent == cur || procs[parent].2 > created {
                break;
            }
            if hit[parent] {
                hit[i] = true;
                break;
            }
            cur = parent;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(q: &str, name: &str) -> bool {
        Query::parse(q).matches(&name.to_lowercase(), 42)
    }

    #[test]
    fn words_in_any_order() {
        assert!(m("edge micro", "MicrosoftEdge.exe"));
        assert!(!m("edge micro", "msedge.exe"));
        assert!(m("EXCEL", "excel.exe"));
    }

    #[test]
    fn alternatives() {
        assert!(m("excel | code", "Code.exe"));
        assert!(m("excel|code", "EXCEL.EXE"));
        assert!(!m("excel | code", "notepad.exe"));
        assert!(!m("excel |", "notepad.exe"));
    }

    #[test]
    fn exclude_exact_prefix_pid() {
        assert!(!m("code !helper", "code helper.exe"));
        assert!(m("code -helper", "code.exe"));
        assert!(m("=code", "code.exe"));
        assert!(!m("=code", "vscode.exe"));
        assert!(m("^py", "python.exe"));
        assert!(!m("^py", "spy.exe"));
        assert!(m("pid:42", "anything.exe"));
        assert!(m("\"code helper\"", "Code Helper.exe"));
        assert!(m("!svchost", "explorer.exe"));
        assert!(!m("!svchost", "svchost.exe"));
    }

    #[test]
    fn children() {
        // 10 -> 11 -> 12, and 13 whose parent PID 10 was reused (13 is older).
        let procs = [(10, 1, 100), (11, 10, 110), (12, 11, 120), (13, 10, 50), (20, 1, 5)];
        let mut hit = [true, false, false, false, false];
        add_children(&procs, &mut hit);
        assert_eq!(hit, [true, true, true, false, false]);
    }

    #[test]
    fn empty() {
        assert!(Query::parse("  | ").is_empty());
        assert!(!Query::parse("a").is_empty());
    }
}
