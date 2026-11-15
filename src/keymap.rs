//! Keyboard shortcuts: every action with its keymap.ini name, palette title and
//! default keys. keymap.ini in the settings folder overrides the defaults.

use std::collections::HashMap;
use std::fmt::Write;

use crate::columns::{self, Col};
use crate::ini;

pub const FILE: &str = "keymap.ini";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Action {
    Palette,
    Help,
    Quit,
    FocusSearch,
    ClearSearch,
    NextPane,
    PrevPane,
    FocusWatch,
    FocusAll,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Kill,
    KillTree,
    KillWatched,
    AddToWatch,
    CopyRow,
    CopyPath,
    OpenLocation,
    Sort(Col),
    ToggleColumn(Col),
    CpuMode,
    ToggleTheme,
    Pause,
    RefreshNow,
    Faster,
    Slower,
    WatchChildren,
    WatchGrow,
    WatchShrink,
    FontBigger,
    FontSmaller,
    FontReset,
    AlwaysOnTop,
    ToggleGpu,
    ToggleHeat,
    ToggleCompact,
    RestartAdmin,
    WriteKeymap,
    WriteSettings,
    WriteTheme,
    OpenConfigFolder,
    ReloadConfig,
    /// 0 idle, 1 below normal, 2 normal, 3 above normal, 4 high.
    Priority(u8),
}

pub struct Binding {
    pub group: &'static str,
    pub name: String,
    pub title: String,
    pub action: Action,
    pub default: String,
}

pub const PRIORITIES: [&str; 5] = ["Idle", "Below normal", "Normal", "Above normal", "High"];

/// Every action in the order keymap.ini and the palette list them.
pub fn bindings() -> Vec<Binding> {
    let mut v = Vec::new();
    let mut add = |group: &'static str, name: &str, title: &str, action: Action, default: &str| {
        v.push(Binding {
            group,
            name: name.to_string(),
            title: title.to_string(),
            action,
            default: default.to_string(),
        })
    };
    use Action::*;
    let g = "General";
    add(g, "palette", "Show all commands", Palette, "Ctrl+Shift+P, Ctrl+K");
    add(g, "help", "Help: search syntax and keys", Help, "F1");
    add(g, "quit", "Quit", Quit, "Ctrl+Q");
    add(g, "toggle_theme", "Toggle dark / light theme", ToggleTheme, "Ctrl+T");
    add(g, "font_bigger", "Font: larger", FontBigger, "Ctrl+=, Ctrl+Shift+=, Ctrl+NumAdd");
    add(g, "font_smaller", "Font: smaller", FontSmaller, "Ctrl+-, Ctrl+NumSubtract");
    add(g, "font_reset", "Font: default size", FontReset, "Ctrl+0, Ctrl+Num0");
    add(g, "always_on_top", "Toggle always on top", AlwaysOnTop, "");
    add(g, "restart_admin", "Restart as administrator", RestartAdmin, "");

    let g = "Moving around";
    add(g, "focus_search", "Focus the watch search box", FocusSearch, "Ctrl+F, Ctrl+L, /");
    add(g, "clear_search", "Clear the watch search", ClearSearch, "");
    add(g, "next_pane", "Next area", NextPane, "Tab");
    add(g, "prev_pane", "Previous area", PrevPane, "Shift+Tab");
    add(g, "focus_watch", "Focus the watch table", FocusWatch, "Alt+1");
    add(g, "focus_all", "Focus the all-processes table", FocusAll, "Alt+2");
    add(g, "up", "Select previous row", Up, "Up");
    add(g, "down", "Select next row", Down, "Down");
    add(g, "page_up", "Page up", PageUp, "PageUp");
    add(g, "page_down", "Page down", PageDown, "PageDown");
    add(g, "top", "First row", Top, "Home");
    add(g, "bottom", "Last row", Bottom, "End");

    let g = "Processes";
    add(g, "kill", "End the selected process", Kill, "Delete");
    add(g, "kill_tree", "End the selected process and its children", KillTree, "Shift+Delete");
    add(g, "kill_watched", "End every process in the watch table", KillWatched, "Ctrl+Shift+K");
    add(g, "add_to_watch", "Add the selected process to the watch search", AddToWatch, "Insert, Ctrl+Enter");
    add(g, "copy_row", "Copy name, PID and path", CopyRow, "Ctrl+C");
    add(g, "copy_path", "Copy the executable path", CopyPath, "Ctrl+Shift+C");
    add(g, "open_location", "Open file location", OpenLocation, "Ctrl+E");
    for (i, p) in PRIORITIES.iter().enumerate() {
        let name = format!("priority_{}", p.to_ascii_lowercase().replace(' ', "_"));
        add(g, &name, &format!("Set priority: {p}"), Priority(i as u8), "");
    }

    let g = "View";
    add(g, "cpu_mode", "Toggle CPU %: per core / whole machine", CpuMode, "Alt+C");
    add(g, "watch_children", "Toggle watching child processes too", WatchChildren, "Alt+T");
    add(g, "watch_grow", "Make the watch table taller", WatchGrow, "Alt+Down");
    add(g, "watch_shrink", "Make the watch table shorter", WatchShrink, "Alt+Up");
    add(g, "pause", "Pause / resume updates", Pause, "Pause, F9");
    add(g, "refresh_now", "Refresh now", RefreshNow, "F5");
    add(g, "refresh_faster", "Update faster", Faster, "Ctrl+Shift+Up");
    add(g, "refresh_slower", "Update slower", Slower, "Ctrl+Shift+Down");
    add(g, "toggle_gpu", "Toggle GPU sampling", ToggleGpu, "");
    add(g, "toggle_heat", "Toggle CPU / GPU bars", ToggleHeat, "");
    add(g, "toggle_compact", "Toggle compact summary (text only, no graphs)", ToggleCompact, "Alt+S");

    let g = "Sorting";
    let sort_keys: HashMap<Col, String> = columns::DEFAULT
        .iter()
        .take(9)
        .enumerate()
        .map(|(i, c)| (*c, format!("Ctrl+{}", i + 1)))
        .collect();
    for c in columns::ALL {
        add(
            g,
            &format!("sort_{}", c.key()),
            &format!("Sort by {}", c.title()),
            Sort(c),
            sort_keys.get(&c).map(|s| s.as_str()).unwrap_or(""),
        );
    }

    let g = "Columns";
    for c in columns::ALL {
        if c != Col::Name {
            add(g, &format!("column_{}", c.key()), &format!("Show / hide column: {}", c.title()), ToggleColumn(c), "");
        }
    }

    let g = "Settings files";
    add(g, "write_keymap", "Write keymap.ini (all keys) and open it", WriteKeymap, "");
    add(g, "write_settings", "Write settings.ini (all options) and open it", WriteSettings, "");
    add(g, "write_theme", "Write theme.ini (all colors) and open it", WriteTheme, "");
    add(g, "open_config_folder", "Open the settings folder", OpenConfigFolder, "");
    add(g, "reload_config", "Reload settings, keys and theme", ReloadConfig, "");
    v
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Key {
    pub vk: u16,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

const NAMED: &[(u16, &str)] = &[
    (0x08, "Backspace"),
    (0x09, "Tab"),
    (0x0D, "Enter"),
    (0x13, "Pause"),
    (0x1B, "Escape"),
    (0x20, "Space"),
    (0x21, "PageUp"),
    (0x22, "PageDown"),
    (0x23, "End"),
    (0x24, "Home"),
    (0x25, "Left"),
    (0x26, "Up"),
    (0x27, "Right"),
    (0x28, "Down"),
    (0x2D, "Insert"),
    (0x2E, "Delete"),
    (0x5D, "Apps"),
    (0x6A, "NumMultiply"),
    (0x6B, "NumAdd"),
    (0x6D, "NumSubtract"),
    (0x6E, "NumDecimal"),
    (0x6F, "NumDivide"),
    (0xBA, ";"),
    (0xBB, "="),
    (0xBC, ","),
    (0xBD, "-"),
    (0xBE, "."),
    (0xBF, "/"),
    (0xC0, "`"),
    (0xDB, "["),
    (0xDC, "\\"),
    (0xDD, "]"),
    (0xDE, "'"),
];

const ALIASES: &[(&str, u16)] = &[
    ("esc", 0x1B),
    ("return", 0x0D),
    ("del", 0x2E),
    ("ins", 0x2D),
    ("pgup", 0x21),
    ("pgdn", 0x22),
    ("plus", 0xBB),
    ("equals", 0xBB),
    ("minus", 0xBD),
    ("comma", 0xBC),
    ("period", 0xBE),
    ("slash", 0xBF),
    ("backslash", 0xDC),
    ("semicolon", 0xBA),
    ("quote", 0xDE),
    ("backtick", 0xC0),
    ("menu", 0x5D),
];

pub fn vk_name(vk: u16) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => NAMED
            .iter()
            .find(|(v, _)| *v == vk)
            .map(|(_, n)| n.to_string())
            .unwrap_or_else(|| format!("0x{vk:02X}")),
    }
}

fn parse_vk(s: &str) -> Option<u16> {
    let l = s.to_ascii_lowercase();
    if s.len() == 1 {
        let c = s.as_bytes()[0].to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            return Some(c as u16);
        }
    }
    if let Some(n) = l.strip_prefix('f').and_then(|n| n.parse::<u16>().ok())
        && (1..=24).contains(&n)
    {
        return Some(0x6F + n);
    }
    if let Some(n) = l.strip_prefix("num").and_then(|n| n.parse::<u16>().ok())
        && n <= 9
    {
        return Some(0x60 + n);
    }
    if let Some((v, _)) = NAMED.iter().find(|(_, n)| n.eq_ignore_ascii_case(s)) {
        return Some(*v);
    }
    if let Some(n) = l.strip_prefix("0x").and_then(|h| u16::from_str_radix(h, 16).ok()) {
        return Some(n);
    }
    ALIASES.iter().find(|(n, _)| *n == l).map(|(_, v)| *v)
}

impl Key {
    /// "Ctrl+Shift+P". A lone "+" key is not supported; use "=" with Shift or "Plus".
    pub fn parse(s: &str) -> Option<Key> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let mut k = Key { vk: 0, ctrl: false, shift: false, alt: false };
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let (last, mods) = parts.split_last()?;
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => k.ctrl = true,
                "shift" => k.shift = true,
                "alt" => k.alt = true,
                _ => return None,
            }
        }
        k.vk = parse_vk(last)?;
        Some(k)
    }

    pub fn name(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        s.push_str(&vk_name(self.vk));
        s
    }
}

fn parse_list(s: &str) -> Vec<Key> {
    s.split(',').filter_map(Key::parse).collect()
}

pub struct Keymap {
    map: HashMap<Key, Action>,
    /// Each action's keys, in `bindings()` order.
    keys: Vec<(Action, Vec<Key>)>,
    pub errors: Vec<String>,
}

impl Keymap {
    pub fn load() -> Keymap {
        let mut overrides: HashMap<String, String> = HashMap::new();
        let mut errors = Vec::new();
        for (section, k, v) in ini::read(&ini::config_path(FILE)) {
            if section == "keys" || section.is_empty() {
                overrides.insert(k, v);
            }
        }
        let all = bindings();
        for k in overrides.keys() {
            if !all.iter().any(|b| &b.name == k) {
                errors.push(format!("keymap.ini: unknown action \"{k}\""));
            }
        }
        let mut keys = Vec::with_capacity(all.len());
        let mut map = HashMap::new();
        // Defaults first, then the user's keys, so a key the user assigns wins.
        let mut user = Vec::new();
        for b in &all {
            match overrides.get(&b.name) {
                Some(v) => {
                    let list = parse_list(v);
                    for part in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                        if Key::parse(part).is_none() {
                            errors.push(format!("keymap.ini: can't read key \"{part}\" for {}", b.name));
                        }
                    }
                    user.push((b.action, list.clone()));
                    keys.push((b.action, list));
                }
                None => {
                    let list = parse_list(&b.default);
                    for k in &list {
                        map.insert(*k, b.action);
                    }
                    keys.push((b.action, list));
                }
            }
        }
        for (a, list) in user {
            for k in list {
                map.insert(k, a);
            }
        }
        // A default key taken over by the user's binding no longer belongs to its old action.
        for (a, list) in &mut keys {
            list.retain(|k| map.get(k) == Some(a));
        }
        Keymap { map, keys, errors }
    }

    pub fn lookup(&self, k: Key) -> Option<Action> {
        self.map.get(&k).copied()
    }

    pub fn keys_for(&self, a: Action) -> &[Key] {
        self.keys
            .iter()
            .find(|(x, _)| *x == a)
            .map(|(_, k)| k.as_slice())
            .unwrap_or(&[])
    }

    /// "Ctrl+Shift+P, Ctrl+K" or "".
    pub fn label(&self, a: Action) -> String {
        self.keys_for(a).iter().map(|k| k.name()).collect::<Vec<_>>().join(", ")
    }

    /// The first key only, for compact hints.
    pub fn short_label(&self, a: Action) -> String {
        self.keys_for(a).first().map(|k| k.name()).unwrap_or_default()
    }

    /// keymap.ini with every action and its current keys.
    pub fn to_ini(&self) -> String {
        let mut s = String::new();
        s.push_str("; winproc-gui keyboard shortcuts.\r\n");
        s.push_str(";\r\n");
        s.push_str("; action = Key, Another+Key      (an empty value leaves the action unbound)\r\n");
        s.push_str("; Modifiers: Ctrl, Shift, Alt. Keys: A-Z, 0-9, F1-F24, Up, Down, Left, Right,\r\n");
        s.push_str("; PageUp, PageDown, Home, End, Insert, Delete, Enter, Escape, Tab, Space,\r\n");
        s.push_str("; Backspace, Pause, Apps, Num0-Num9, NumAdd, NumSubtract, and = - , . / ; ' [ ] \\ `\r\n");
        s.push_str(";\r\n");
        s.push_str("; Text boxes keep their own editing keys (arrows, Home/End, Ctrl+A/C/V/X,\r\n");
        s.push_str("; Ctrl+Backspace) while they have focus. Save this file and switch back to the\r\n");
        s.push_str("; app to load it.\r\n\r\n[keys]\r\n");
        let mut group = "";
        for b in bindings() {
            if b.group != group {
                group = b.group;
                let _ = write!(s, "\r\n; --- {group} ---\r\n");
            }
            let _ = write!(s, "; {}\r\n{} = {}\r\n", b.title, b.name, self.label(b.action));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_name() {
        let k = Key::parse("ctrl+shift+p").unwrap();
        assert_eq!(k.name(), "Ctrl+Shift+P");
        assert_eq!(Key::parse("Ctrl+=").unwrap().vk, 0xBB);
        assert_eq!(Key::parse("F12").unwrap().vk, 0x7B);
        assert_eq!(Key::parse("Del").unwrap().vk, 0x2E);
        assert_eq!(Key::parse("/").unwrap().vk, 0xBF);
        assert_eq!(Key::parse("Ctrl+-").unwrap().name(), "Ctrl+-");
        assert!(Key::parse("Hyper+X").is_none());
    }

    #[test]
    fn names_are_unique() {
        let b = bindings();
        let mut names: Vec<&str> = b.iter().map(|b| b.name.as_str()).collect();
        names.sort();
        let n = names.len();
        names.dedup();
        assert_eq!(n, names.len());
        for b in &b {
            for part in b.default.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                assert!(Key::parse(part).is_some(), "{part}");
            }
        }
    }
}
