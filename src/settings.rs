//! settings.ini (options people edit) and state.ini (what the app remembers on its
//! own: window placement, the watch search, sort order).

use std::fmt::Write;
use std::path::PathBuf;

use crate::columns::{self, Col};
use crate::ini;

pub const FILE: &str = "settings.ini";
pub const STATE_FILE: &str = "state.ini";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemeChoice {
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CpuMode {
    /// 100% is one core, like Linux `top`. A busy process can show 400% on a 4-core machine.
    PerCore,
    /// 100% is the whole machine, like Task Manager.
    Total,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub theme: ThemeChoice,
    pub font: String,
    pub font_size: f32,
    pub refresh_ms: u32,
    pub cpu_mode: CpuMode,
    pub columns: Vec<Col>,
    pub gpu: bool,
    pub confirm_kill: bool,
    pub watch_children: bool,
    pub always_on_top: bool,
    pub pause_when_minimized: bool,
    pub show_idle: bool,
    pub heat_bars: bool,
    pub hardware_rendering: bool,
    pub history: usize,
}

pub const DEFAULT_FONT_SIZE: f32 = 13.0;

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::System,
            font: "Segoe UI".into(),
            font_size: DEFAULT_FONT_SIZE,
            refresh_ms: 1000,
            cpu_mode: CpuMode::PerCore,
            columns: columns::DEFAULT.to_vec(),
            gpu: true,
            confirm_kill: true,
            watch_children: false,
            always_on_top: false,
            pause_when_minimized: true,
            show_idle: false,
            heat_bars: true,
            hardware_rendering: false,
            history: 60,
        }
    }
}

pub fn path() -> PathBuf {
    ini::config_path(FILE)
}

impl Settings {
    pub fn load() -> Settings {
        let mut s = Settings::default();
        for (section, k, v) in ini::read(&path()) {
            if section == "settings" {
                s.apply(&k, &v);
            }
        }
        s
    }

    fn apply(&mut self, k: &str, v: &str) {
        let b = ini::parse_bool(v);
        match k {
            "theme" => {
                self.theme = match v.to_ascii_lowercase().as_str() {
                    "dark" => ThemeChoice::Dark,
                    "light" => ThemeChoice::Light,
                    _ => ThemeChoice::System,
                }
            }
            "font" if !v.is_empty() => self.font = v.to_string(),
            "font_size" => {
                if let Ok(n) = v.parse::<f32>() {
                    self.font_size = n.clamp(7.0, 40.0);
                }
            }
            "refresh_ms" => {
                if let Ok(n) = v.parse::<u32>() {
                    self.refresh_ms = n.clamp(100, 60_000);
                }
            }
            "cpu_mode" => {
                self.cpu_mode = if v.eq_ignore_ascii_case("total") { CpuMode::Total } else { CpuMode::PerCore }
            }
            "columns" => self.columns = columns::parse_list(v),
            "gpu" => self.gpu = b.unwrap_or(self.gpu),
            "confirm_kill" => self.confirm_kill = b.unwrap_or(self.confirm_kill),
            "watch_children" => self.watch_children = b.unwrap_or(self.watch_children),
            "always_on_top" => self.always_on_top = b.unwrap_or(self.always_on_top),
            "pause_when_minimized" => self.pause_when_minimized = b.unwrap_or(self.pause_when_minimized),
            "show_idle" => self.show_idle = b.unwrap_or(self.show_idle),
            "heat_bars" => self.heat_bars = b.unwrap_or(self.heat_bars),
            "hardware_rendering" => self.hardware_rendering = b.unwrap_or(self.hardware_rendering),
            "history" => {
                if let Ok(n) = v.parse::<usize>() {
                    self.history = n.clamp(10, 600);
                }
            }
            _ => {}
        }
    }

    pub fn theme_str(&self) -> &'static str {
        match self.theme {
            ThemeChoice::System => "system",
            ThemeChoice::Dark => "dark",
            ThemeChoice::Light => "light",
        }
    }

    pub fn cpu_mode_str(&self) -> &'static str {
        match self.cpu_mode {
            CpuMode::PerCore => "per_core",
            CpuMode::Total => "total",
        }
    }

    /// settings.ini with every option, its current value, and what it does.
    pub fn to_ini(&self) -> String {
        let mut s = String::from(
            "; winproc-gui settings. Edit and save; the app reloads this file when it gets focus.\r\n\
             ; Delete a line to go back to the default.\r\n\r\n[settings]\r\n\r\n",
        );
        let yn = |b: bool| if b { "true".to_string() } else { "false".to_string() };
        let mut put = |doc: &str, k: &str, v: String| {
            for line in doc.lines() {
                let _ = write!(s, "; {line}\r\n");
            }
            let _ = write!(s, "{k} = {v}\r\n\r\n");
        };
        put("system, dark or light. system follows the Windows app mode.", "theme", self.theme_str().into());
        put("Font for everything in the window.", "font", self.font.clone());
        put("Font size in points (Ctrl+= and Ctrl+- change it).", "font_size", format!("{}", self.font_size));
        put("How often to sample, in milliseconds.", "refresh_ms", self.refresh_ms.to_string());
        put(
            "per_core: 100% is one core (a busy process can go past 100%).\ntotal: 100% is the whole machine, like Task Manager.",
            "cpu_mode",
            self.cpu_mode_str().into(),
        );
        let mut doc = String::from("Columns in both process tables, in order. Available:\n");
        for c in columns::ALL {
            let _ = writeln!(doc, "  {:<12} {}", c.key(), c.describe());
        }
        put(&doc, "columns", columns::list_to_string(&self.columns));
        put(
            "Sample GPU usage. Reading GPU counters costs more CPU than everything else\ntogether, so turn it off if you do not need it.",
            "gpu",
            yn(self.gpu),
        );
        put("Ask before ending processes.", "confirm_kill", yn(self.confirm_kill));
        put(
            "Also list the child processes of watched processes (for example the\nhelpers a browser or editor starts).",
            "watch_children",
            yn(self.watch_children),
        );
        put("Keep the window above other windows.", "always_on_top", yn(self.always_on_top));
        put("Stop sampling while the window is minimized.", "pause_when_minimized", yn(self.pause_when_minimized));
        put("List the System Idle Process.", "show_idle", yn(self.show_idle));
        put("Draw bars behind CPU and GPU values in the tables.", "heat_bars", yn(self.heat_bars));
        put(
            "Draw with the GPU instead of the CPU. Software drawing starts faster and\nuses less memory; hardware drawing can be smoother on very large windows.\nTakes effect on restart.",
            "hardware_rendering",
            yn(self.hardware_rendering),
        );
        put("Samples kept for the summary graphs.", "history", self.history.to_string());
        s
    }
}

/// Changes one option in settings.ini, keeping the rest of the file as it is.
pub fn save_key(key: &str, value: &str) {
    let _ = ini::set_key(&path(), "settings", key, value);
}

#[derive(Clone, Debug)]
pub struct SortState {
    pub col: Col,
    pub desc: bool,
}

#[derive(Clone, Debug)]
pub struct State {
    /// Normal (restored) window rectangle in screen pixels.
    pub rect: Option<(i32, i32, i32, i32)>,
    pub maximized: bool,
    pub search: String,
    pub history: Vec<String>,
    pub watch_sort: SortState,
    pub all_sort: SortState,
    /// Height of the watch table in DIPs at font size 13.
    pub watch_height: f32,
}

impl Default for State {
    fn default() -> Self {
        State {
            rect: None,
            maximized: false,
            search: String::new(),
            history: Vec::new(),
            watch_sort: SortState { col: Col::Name, desc: false },
            all_sort: SortState { col: Col::Cpu, desc: true },
            watch_height: 170.0,
        }
    }
}

fn parse_sort(v: &str) -> Option<SortState> {
    let mut it = v.split_whitespace();
    let col = Col::from_key(it.next()?)?;
    let desc = it.next().is_some_and(|d| d.eq_ignore_ascii_case("desc"));
    Some(SortState { col, desc })
}

impl State {
    pub fn load() -> State {
        let mut s = State::default();
        let mut rect = [None; 4];
        for (section, k, v) in ini::read(&ini::config_path(STATE_FILE)) {
            match (section.as_str(), k.as_str()) {
                ("window", "x") => rect[0] = v.parse().ok(),
                ("window", "y") => rect[1] = v.parse().ok(),
                ("window", "width") => rect[2] = v.parse().ok(),
                ("window", "height") => rect[3] = v.parse().ok(),
                ("window", "maximized") => s.maximized = ini::parse_bool(&v).unwrap_or(false),
                ("window", "watch_height") => {
                    if let Ok(h) = v.parse::<f32>() {
                        s.watch_height = h.clamp(40.0, 4000.0);
                    }
                }
                ("watch", "search") => s.search = unescape(&v),
                ("watch", k) if k.starts_with("history") => {
                    let v = unescape(&v);
                    if !v.trim().is_empty() {
                        s.history.push(v);
                    }
                }
                ("sort", "watch") => s.watch_sort = parse_sort(&v).unwrap_or(s.watch_sort),
                ("sort", "all") => s.all_sort = parse_sort(&v).unwrap_or(s.all_sort),
                _ => {}
            }
        }
        if let [Some(x), Some(y), Some(w), Some(h)] = rect
            && w > 100
            && h > 100
        {
            s.rect = Some((x, y, w, h));
        }
        s
    }

    pub fn save(&self) {
        let mut s = String::from("; winproc-gui remembers these between runs. Safe to delete.\r\n\r\n[window]\r\n");
        if let Some((x, y, w, h)) = self.rect {
            let _ = write!(s, "x = {x}\r\ny = {y}\r\nwidth = {w}\r\nheight = {h}\r\n");
        }
        let _ = write!(s, "maximized = {}\r\nwatch_height = {:.0}\r\n", self.maximized, self.watch_height);
        let _ = write!(s, "\r\n[watch]\r\nsearch = {}\r\n", escape(&self.search));
        for (i, h) in self.history.iter().enumerate() {
            let _ = write!(s, "history{i} = {}\r\n", escape(h));
        }
        let sort = |st: &SortState| format!("{} {}", st.col.key(), if st.desc { "desc" } else { "asc" });
        let _ = write!(s, "\r\n[sort]\r\nwatch = {}\r\nall = {}\r\n", sort(&self.watch_sort), sort(&self.all_sort));
        let path = ini::config_path(STATE_FILE);
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        // Write then rename, so a crash mid-write never leaves a half file.
        let tmp = path.with_extension("ini.tmp");
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Values are trimmed when read, so leading/trailing spaces are kept in quotes.
fn escape(s: &str) -> String {
    if s.starts_with(' ') || s.ends_with(' ') || s.starts_with('"') {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

fn unescape(s: &str) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}
