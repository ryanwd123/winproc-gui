//! A tiny INI reader and an in-place key writer.
//!
//! Format: `[section]` headers, `key = value` lines, and `;` or `#` comments.
//! Keys and sections are case-insensitive; values keep their case.

use std::path::{Path, PathBuf};

/// Every `(section, key, value)` in the text, with section and key lowercased.
pub fn parse(text: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            section = rest.trim_end_matches(']').trim().to_ascii_lowercase();
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.push((
                section.clone(),
                k.trim().to_ascii_lowercase(),
                v.trim().to_string(),
            ));
        }
    }
    out
}

/// Reads and parses a file; a missing or unreadable file gives no entries.
pub fn read(path: &Path) -> Vec<(String, String, String)> {
    std::fs::read_to_string(path)
        .map(|t| parse(t.trim_start_matches('\u{feff}')))
        .unwrap_or_default()
}

/// Sets `key = value` in `section`, keeping the rest of the file (and its comments)
/// as it is. Adds the key or the section when they are missing, and creates the
/// file when it does not exist.
pub fn set_key(path: &Path, section: &str, key: &str, value: &str) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let new = set_key_in(&text, section, key, value);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, new)
}

fn set_key_in(text: &str, section: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut cur = String::new();
    let mut section_end: Option<usize> = None; // index after the last line of `section`
    for (i, line) in lines.iter_mut().enumerate() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('[') {
            cur = rest.trim_end_matches(']').trim().to_ascii_lowercase();
            if cur == section {
                section_end = Some(i + 1);
            }
            continue;
        }
        if cur != section {
            continue;
        }
        if !t.is_empty() && !t.starts_with(';') && !t.starts_with('#') {
            section_end = Some(i + 1);
        }
        if let Some((k, _)) = t.split_once('=')
            && k.trim().eq_ignore_ascii_case(key)
            && !t.starts_with(';')
            && !t.starts_with('#')
        {
            *line = format!("{key} = {value}");
            return join(lines);
        }
    }
    match section_end {
        Some(at) => lines.insert(at, format!("{key} = {value}")),
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{section}]"));
            lines.push(format!("{key} = {value}"));
        }
    }
    join(lines)
}

fn join(lines: Vec<String>) -> String {
    let mut s = lines.join("\r\n");
    s.push_str("\r\n");
    s
}

/// `%APPDATA%\winproc-gui`, the folder for settings.ini, keymap.ini, theme.ini
/// and state.ini.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("winproc-gui")
}

pub fn config_path(name: &str) -> PathBuf {
    config_dir().join(name)
}

pub fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_and_comments() {
        let e = parse("; c\n[Keys]\nPalette = Ctrl+P ; not a comment\n# x\nb=1");
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].0, "keys");
        assert_eq!(e[0].1, "palette");
        assert_eq!(e[0].2, "Ctrl+P ; not a comment");
        assert_eq!(e[1].2, "1");
    }

    #[test]
    fn set_key_replaces_in_place() {
        let t = "; top\n[settings]\n; the theme\ntheme = dark\nfont = x\n";
        let n = set_key_in(t, "settings", "theme", "light");
        assert_eq!(n, "; top\r\n[settings]\r\n; the theme\r\ntheme = light\r\nfont = x\r\n");
    }

    #[test]
    fn set_key_adds_missing_key_and_section() {
        let n = set_key_in("[settings]\na = 1\n\n[other]\nb = 2\n", "settings", "c", "3");
        assert_eq!(n, "[settings]\r\na = 1\r\nc = 3\r\n\r\n[other]\r\nb = 2\r\n");
        let n = set_key_in("", "settings", "c", "3");
        assert_eq!(n, "[settings]\r\nc = 3\r\n");
    }
}
