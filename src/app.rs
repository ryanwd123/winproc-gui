//! The window's state, layout, painting, input and commands.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::SystemTime;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;

use crate::columns::Col;
use crate::format;
use crate::ini;
use crate::keymap::{self, Action, Binding, Key, Keymap};
use crate::platform::{self, Target};
use crate::render::{Align, Font, Gfx, Rect, alpha};
use crate::sampler::{self, Proc, Shared, Snapshot};
use crate::search::{self, Query};
use crate::settings::{self, CpuMode, Settings, SortState, State, ThemeChoice};
use crate::textbox::TextBox;
use crate::theme::{self, Theme};

pub const WM_APP_SNAPSHOT: u32 = WM_APP + 1;
const TIMER_STATUS: usize = 1;
const TIMER_REFRESH: usize = 2;
const REFRESH_STEPS: [u32; 6] = [250, 500, 1000, 2000, 5000, 10000];
const HISTORY_MAX: usize = 30;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Search,
    Watch,
    All,
}

/// What the window shows. Ctrl+Tab cycles through them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    /// Summary, watch table and all processes.
    Main,
    /// Summary and the watch table only. The sampler builds rows for watched
    /// processes only, and the all-processes table is not kept up to date.
    Watch,
}

const VIEWS: [View; 2] = [View::Main, View::Watch];

impl View {
    fn key(self) -> &'static str {
        match self {
            View::Main => "main",
            View::Watch => "watch",
        }
    }
    fn title(self) -> &'static str {
        match self {
            View::Main => "Main view",
            View::Watch => "Watch view",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Which {
    Watch,
    All,
}

struct Table {
    /// Indices into the snapshot's processes, in display order.
    rows: Vec<usize>,
    sel: Option<(u32, u64)>,
    /// The selected row's index, kept when the process exits so keys continue from there.
    sel_idx: usize,
    scroll: usize,
    sort: SortState,
    visible: usize,
}

impl Table {
    fn new(sort: SortState) -> Table {
        Table { rows: Vec::new(), sel: None, sel_idx: 0, scroll: 0, sort, visible: 1 }
    }

    fn clamp_scroll(&mut self) {
        let max = self.rows.len().saturating_sub(self.visible);
        self.scroll = self.scroll.min(max);
    }

    fn reveal(&mut self) {
        if self.sel_idx < self.scroll {
            self.scroll = self.sel_idx;
        } else if self.sel_idx >= self.scroll + self.visible {
            self.scroll = self.sel_idx + 1 - self.visible.max(1);
        }
        self.clamp_scroll();
    }
}

enum Overlay {
    None,
    Palette { input: TextBox, items: Vec<usize>, sel: usize, scroll: usize },
    Confirm { title: String, targets: Vec<Target> },
    Help { scroll: usize },
}

#[derive(Default)]
struct History {
    cpu: VecDeque<f32>,
    mem: VecDeque<f32>,
    disk: VecDeque<f32>,
    gpu: VecDeque<f32>,
    recv: VecDeque<f32>,
    send: VecDeque<f32>,
}

fn push(q: &mut VecDeque<f32>, v: f32, cap: usize) {
    q.push_back(v);
    while q.len() > cap {
        q.pop_front();
    }
}

#[derive(Default, Clone, Copy)]
struct Layout {
    pad: f32,
    row_h: f32,
    header_h: f32,
    sb_w: f32,
    tiles: Rect,
    search: Rect,
    hint: Rect,
    watch: Rect,
    bar: Rect,
    all: Rect,
    status: Rect,
    palette: Rect,
}

enum Drag {
    None,
    Splitter { start_y: f32, start_h: f32 },
    Thumb { which: Which, grab: f32 },
}

pub struct App {
    pub hwnd: HWND,
    pub dpi: f32,
    gfx: Option<Gfx>,
    settings: Settings,
    keymap: Keymap,
    bindings: Vec<Binding>,
    theme: Theme,
    shared: Arc<Shared>,
    snap: Option<Snapshot>,
    query: Query,
    search: TextBox,
    history: Vec<String>,
    hist_pos: Option<usize>,
    hist_saved: String,
    focus: Focus,
    view: View,
    /// The filter last sent to the sampler.
    sent_filter: Option<sampler::Filter>,
    /// Whether the sampler was last told to read the GPU counters.
    sent_gpu: bool,
    watch: Table,
    all: Table,
    watch_height: f32,
    overlay: Overlay,
    graphs: History,
    layout: Layout,
    drag: Drag,
    hover: Option<(Which, usize)>,
    tracking_mouse: bool,
    status: Option<(String, bool)>,
    user_paused: bool,
    minimized: bool,
    pub suppress_char: bool,
    mtimes: [Option<SystemTime>; 3],
    placement: (Option<(i32, i32, i32, i32)>, bool),
}

fn mtime(name: &str) -> Option<SystemTime> {
    std::fs::metadata(ini::config_path(name)).and_then(|m| m.modified()).ok()
}

fn config_mtimes() -> [Option<SystemTime>; 3] {
    [mtime(settings::FILE), mtime(keymap::FILE), mtime(theme::FILE)]
}

fn effective_dark(s: &Settings) -> bool {
    match s.theme {
        ThemeChoice::System => theme::system_is_dark(),
        ThemeChoice::Dark => true,
        ThemeChoice::Light => false,
    }
}

fn mods() -> (bool, bool, bool) {
    unsafe {
        (
            GetKeyState(VK_CONTROL.0 as i32) < 0,
            GetKeyState(VK_SHIFT.0 as i32) < 0,
            GetKeyState(VK_MENU.0 as i32) < 0,
        )
    }
}

/// Scores `s` against the palette query; None when it does not match.
fn fuzzy(q: &str, s: &str) -> Option<i32> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    let s = s.to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    if words.iter().all(|w| s.contains(w)) {
        return Some(1000 - s.find(words[0]).unwrap_or(0) as i32);
    }
    let mut it = s.chars();
    let mut gaps = 0;
    for c in q.chars().filter(|c| !c.is_whitespace()) {
        let mut found = false;
        for d in it.by_ref() {
            if d == c {
                found = true;
                break;
            }
            gaps += 1;
        }
        if !found {
            return None;
        }
    }
    Some(500 - gaps)
}

impl App {
    pub fn new(hwnd: HWND, dpi: f32) -> Box<App> {
        let settings = Settings::load();
        let state = State::load();
        let keymap = Keymap::load();
        let theme = theme::load(effective_dark(&settings));
        let gfx = Gfx::new(hwnd, &settings.font, settings.font_size, settings.hardware_rendering, dpi).ok();
        let shared = sampler::start(
            hwnd,
            WM_APP_SNAPSHOT,
            sampler::Control {
                interval_ms: settings.refresh_ms,
                paused: false,
                gpu: settings.gpu,
                quit: false,
                wake: false,
                filter: None,
                peek: false,
            },
        );
        let search = TextBox::new(&state.search);
        let query = Query::parse(&state.search);
        let focus = if query.is_empty() { Focus::Search } else { Focus::Watch };
        let errors = keymap.errors.clone();
        let mut app = Box::new(App {
            hwnd,
            dpi,
            gfx,
            keymap,
            bindings: keymap::bindings(),
            theme,
            shared,
            snap: None,
            query,
            search,
            history: state.history.clone(),
            hist_pos: None,
            hist_saved: String::new(),
            focus,
            view: if state.view == "watch" { View::Watch } else { View::Main },
            sent_filter: None,
            sent_gpu: settings.gpu,
            watch: Table::new(state.watch_sort.clone()),
            all: Table::new(state.all_sort.clone()),
            watch_height: state.watch_height,
            overlay: Overlay::None,
            graphs: History::default(),
            layout: Layout::default(),
            drag: Drag::None,
            hover: None,
            tracking_mouse: false,
            status: None,
            user_paused: false,
            minimized: false,
            suppress_char: false,
            mtimes: config_mtimes(),
            placement: (state.rect, state.maximized),
            settings,
        });
        app.style_window();
        app.sync_filter();
        if let Some(e) = errors.first() {
            app.flash(e.clone(), true);
        }
        app
    }

    /// The saved window rectangle and maximized flag, for the first show.
    pub fn saved_placement(&self) -> (Option<(i32, i32, i32, i32)>, bool) {
        self.placement
    }

    fn style_window(&self) {
        platform::style_titlebar(self.hwnd, self.theme.dark, self.theme.panel, self.theme.text);
        if self.settings.always_on_top {
            platform::set_topmost(self.hwnd, true);
        }
    }

    pub fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn flash(&mut self, msg: impl Into<String>, error: bool) {
        self.status = Some((msg.into(), error));
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_STATUS, if error { 8000 } else { 4000 }, None);
        }
        self.invalidate();
    }

    pub fn on_timer(&mut self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), id);
        }
        match id {
            TIMER_STATUS => {
                self.status = None;
                self.invalidate();
            }
            TIMER_REFRESH => self.shared.update(|_| {}),
            _ => {}
        }
    }

    // ---------------------------------------------------------------- data

    pub fn on_snapshot(&mut self) {
        let Some(snap) = self.shared.take() else { return };
        if snap.filtered && self.view == View::Main {
            // Taken before the switch to the main view; a full one is on its way.
            return;
        }
        let cap = self.settings.history;
        let s = &snap.sys;
        // Watch-view samples carry no summary values.
        let graph = !snap.peek && !snap.filtered;
        if snap.rates_valid && graph {
            push(&mut self.graphs.cpu, s.cpu / 100.0, cap);
            push(&mut self.graphs.disk, s.disk.as_ref().map(|d| d.active / 100.0).unwrap_or(0.0), cap);
            push(&mut self.graphs.gpu, s.gpu.unwrap_or(0.0) / 100.0, cap);
            push(&mut self.graphs.recv, s.net.recv as f32, cap);
            push(&mut self.graphs.send, s.net.send as f32, cap);
        }
        if s.mem_total > 0 && graph {
            push(&mut self.graphs.mem, 1.0 - s.mem_avail as f32 / s.mem_total as f32, cap);
        }
        self.snap = Some(snap);
        self.rebuild();
        self.invalidate();
    }

    fn cpu_scale(&self) -> f32 {
        match (self.settings.cpu_mode, &self.snap) {
            (CpuMode::Total, Some(s)) => 1.0 / s.sys.ncpu.max(1) as f32,
            _ => 1.0,
        }
    }

    /// Filters and sorts both tables, keeping each selection on its process.
    fn rebuild(&mut self) {
        let Some(snap) = &self.snap else { return };
        let procs = &snap.procs;
        let show_idle = self.settings.show_idle;
        let base = (0..procs.len()).filter(|&i| show_idle || procs[i].pid != 0);

        let mut watched: Vec<usize> = Vec::new();
        if !self.query.is_empty() {
            let mut hit: Vec<bool> = procs.iter().map(|p| self.query.matches(&p.info.name_lower, p.pid)).collect();
            if self.settings.watch_children {
                let tree: Vec<(u32, u32, u64)> = procs.iter().map(|p| (p.pid, p.ppid, p.create_time)).collect();
                search::add_children(&tree, &mut hit);
            }
            watched = base.clone().filter(|&i| hit[i]).collect();
        }
        // The watch view does not keep the all-processes table up to date.
        let full = self.view == View::Main;
        let all: Vec<usize> = if full { base.collect() } else { Vec::new() };

        fn sort(rows: &mut [usize], procs: &[Proc], s: &SortState) {
            rows.sort_by(|&a, &b| {
                let (pa, pb) = (&procs[a], &procs[b]);
                let o = s.col.cmp(pa, pb);
                let o = if s.desc { o.reverse() } else { o };
                o.then_with(|| pa.info.name_lower.cmp(&pb.info.name_lower)).then(pa.pid.cmp(&pb.pid))
            });
        }
        sort(&mut watched, procs, &self.watch.sort);
        let mut all = all;
        sort(&mut all, procs, &self.all.sort);

        for (t, rows, live) in [(&mut self.watch, watched, true), (&mut self.all, all, full)] {
            t.rows = rows;
            // A hidden table keeps its selection for when it is shown again.
            if !live {
                continue;
            }
            if let Some(sel) = t.sel {
                match t.rows.iter().position(|&i| (procs[i].pid, procs[i].create_time) == sel) {
                    Some(i) => t.sel_idx = i,
                    // Gone: select nothing rather than whatever moved into its row.
                    None => t.sel = None,
                }
            }
            t.clamp_scroll();
        }
    }

    fn table(&self, w: Which) -> &Table {
        match w {
            Which::Watch => &self.watch,
            Which::All => &self.all,
        }
    }

    fn table_mut(&mut self, w: Which) -> &mut Table {
        match w {
            Which::Watch => &mut self.watch,
            Which::All => &mut self.all,
        }
    }

    /// The table that keys act on: the watch table while the search box has focus.
    fn active(&self) -> Which {
        match self.focus {
            Focus::All => Which::All,
            _ => Which::Watch,
        }
    }

    fn proc_at(&self, w: Which, row: usize) -> Option<&Proc> {
        let snap = self.snap.as_ref()?;
        self.table(w).rows.get(row).map(|&i| &snap.procs[i])
    }

    fn selected(&self) -> Option<&Proc> {
        let w = self.active();
        let t = self.table(w);
        t.sel?;
        self.proc_at(w, t.sel_idx)
    }

    fn target(p: &Proc) -> Target {
        Target { pid: p.pid, create_time: p.create_time, name: p.info.name.to_string() }
    }

    fn select_row(&mut self, w: Which, row: usize) {
        let id = self.proc_at(w, row).map(|p| (p.pid, p.create_time));
        let t = self.table_mut(w);
        if let Some(id) = id {
            t.sel = Some(id);
            t.sel_idx = row;
            t.reveal();
        }
    }

    fn set_search(&mut self, text: &str) {
        self.search.set(text);
        self.search_changed();
    }

    /// Tells the sampler which processes the current view needs rows for.
    fn sync_filter(&mut self) {
        let filter = match self.view {
            View::Main => None,
            View::Watch => Some(sampler::Filter { query: self.query.clone(), children: self.settings.watch_children }),
        };
        if filter != self.sent_filter {
            let to_main = filter.is_none();
            self.sent_filter = filter.clone();
            if to_main {
                // Watch-view samples skip the summary, so the main view needs a full one now.
                self.shared.update(|c| c.filter = None);
            } else {
                // The sampler rescans right away, so the new rows show up as you type.
                self.shared.set_filter(filter);
            }
        }
        // GPU counters are the most expensive thing sampled; read them only
        // when the summary or the GPU column shows them.
        let gpu = self.settings.gpu && (self.view == View::Main || self.settings.columns.contains(&Col::Gpu));
        if gpu != self.sent_gpu {
            self.sent_gpu = gpu;
            self.shared.update(|c| c.gpu = gpu);
        }
    }

    fn set_view(&mut self, v: View) {
        if v == self.view {
            return;
        }
        self.view = v;
        if v == View::Watch && self.focus == Focus::All {
            self.focus = if self.query.is_empty() { Focus::Search } else { Focus::Watch };
        }
        self.sync_filter();
        // Until the next sample arrives, show what the last one had.
        self.rebuild();
        self.compute_layout();
        self.flash(v.title(), false);
    }

    fn search_changed(&mut self) {
        self.query = Query::parse(&self.search.text);
        self.sync_filter();
        self.watch.scroll = 0;
        self.rebuild();
        self.invalidate();
    }

    fn remember_search(&mut self) {
        let s = self.search.text.trim().to_string();
        self.hist_pos = None;
        if s.is_empty() {
            return;
        }
        self.history.retain(|h| h != &s);
        self.history.insert(0, s);
        self.history.truncate(HISTORY_MAX);
    }

    pub fn save_state(&mut self) {
        self.remember_search();
        let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        let mut rect = self.placement.0;
        let mut maximized = self.placement.1;
        if unsafe { GetWindowPlacement(self.hwnd, &mut wp) }.is_ok() {
            let r = wp.rcNormalPosition;
            rect = Some((r.left, r.top, r.right - r.left, r.bottom - r.top));
            maximized = wp.showCmd == SW_SHOWMAXIMIZED.0 as u32
                || (wp.showCmd == SW_SHOWMINIMIZED.0 as u32 && wp.flags.0 & WPF_RESTORETOMAXIMIZED.0 != 0);
        }
        State {
            rect,
            maximized,
            search: self.search.text.clone(),
            history: self.history.clone(),
            watch_sort: self.watch.sort.clone(),
            all_sort: self.all.sort.clone(),
            watch_height: self.watch_height,
            view: self.view.key().into(),
        }
        .save();
    }

    pub fn shutdown(&mut self) {
        self.save_state();
        self.shared.update(|c| c.quit = true);
    }

    // -------------------------------------------------------------- settings

    fn apply_settings(&mut self, new: Settings) {
        let old = std::mem::replace(&mut self.settings, new);
        let s = self.settings.clone();
        if (old.font != s.font || old.font_size != s.font_size)
            && let Some(g) = &mut self.gfx
        {
            g.set_font(&s.font, s.font_size);
        }
        if old.theme != s.theme {
            self.reload_theme();
        }
        if old.always_on_top != s.always_on_top {
            platform::set_topmost(self.hwnd, s.always_on_top);
        }
        if old.refresh_ms != s.refresh_ms {
            let ms = s.refresh_ms;
            self.shared.update(|c| c.interval_ms = ms);
        }
        self.update_paused();
        self.sync_filter();
        self.rebuild();
        self.compute_layout();
        self.invalidate();
    }

    fn reload_theme(&mut self) {
        self.theme = theme::load(effective_dark(&self.settings));
        self.style_window();
        self.invalidate();
    }

    pub fn reload_config(&mut self, quiet: bool) {
        self.keymap = Keymap::load();
        self.apply_settings(Settings::load());
        self.reload_theme();
        self.mtimes = config_mtimes();
        if let Some(e) = self.keymap.errors.first() {
            self.flash(e.clone(), true);
        } else if !quiet {
            self.flash("Reloaded settings, keys and theme", false);
        }
    }

    /// Called when the window is activated: picks up edits made in a text editor.
    pub fn check_config(&mut self) {
        if config_mtimes() != self.mtimes {
            self.reload_config(false);
        }
    }

    pub fn on_system_theme_change(&mut self) {
        if self.settings.theme == ThemeChoice::System {
            self.reload_theme();
        }
    }

    fn update_paused(&self) {
        let paused = self.user_paused || (self.minimized && self.settings.pause_when_minimized);
        self.shared.update(|c| c.paused = paused);
    }

    fn set_font_size(&mut self, size: f32) {
        let size = size.clamp(7.0, 40.0);
        if size == self.settings.font_size {
            return;
        }
        self.settings.font_size = size;
        if let Some(g) = &mut self.gfx {
            g.set_font(&self.settings.font, size);
        }
        settings::save_key("font_size", &format!("{size}"));
        self.mtimes = config_mtimes();
        self.flash(format!("Font size {size}"), false);
    }

    fn save_setting(&mut self, key: &str, value: &str) {
        settings::save_key(key, value);
        self.mtimes = config_mtimes();
    }

    // ---------------------------------------------------------------- window

    pub fn on_size(&mut self, pw: u32, ph: u32, minimized: bool) {
        if minimized != self.minimized {
            self.minimized = minimized;
            self.update_paused();
        }
        if minimized {
            return;
        }
        let dpi = self.dpi;
        if let Some(g) = &mut self.gfx {
            g.resize(pw, ph, dpi);
        }
        self.compute_layout();
        self.invalidate();
    }

    fn compute_layout(&mut self) {
        let Some(g) = &self.gfx else { return };
        let (w, h, s) = (g.w, g.h, g.size);
        let pad = (s * 0.6).round();
        let row_h = (s * 1.65).round();
        let header_h = row_h + 2.0;
        let mut l = Layout { pad, row_h, header_h, sb_w: (s * 0.75).round(), ..Default::default() };
        let inner = w - 2.0 * pad;
        let mut y = pad;
        if self.view == View::Main {
            let tiles_h = if self.settings.compact_summary { (s * 2.1).round() } else { (s * 5.4).round() };
            l.tiles = Rect::new(pad, y, inner, tiles_h);
            y += l.tiles.h + pad;
        } else {
            // The watch view has no summary bar.
            l.tiles = Rect::new(pad, y, inner, 0.0);
        }
        l.search = Rect::new(pad, y, inner, (s * 2.1).round());
        y += l.search.h + pad * 0.5;
        let status_h = (s * 1.75).round();
        l.status = Rect::new(0.0, h - status_h, w, status_h);
        let bar_h = (s * 1.7).round();
        if self.view == View::Watch {
            let bottom = l.status.y - pad * 0.5;
            if self.query.is_empty() {
                l.hint = Rect::new(pad, y, inner, row_h);
            } else {
                l.watch = Rect::new(pad, y, inner, (bottom - y).max(header_h + row_h * 2.0));
            }
            self.watch.visible = (((l.watch.h - header_h - row_h) / row_h).floor().max(1.0)) as usize;
            self.watch.clamp_scroll();
            let pw = (w - 4.0 * pad).min(s * 50.0);
            l.palette = Rect::new((w - pw) / 2.0, l.tiles.y + pad * 2.0, pw, 0.0);
            self.layout = l;
            return;
        }
        if self.query.is_empty() {
            l.hint = Rect::new(pad, y, inner, row_h);
            y += row_h;
        } else {
            let min = header_h + row_h * 3.0;
            let max = (l.status.y - y - bar_h - header_h - row_h * 4.0).max(min);
            let wh = self.watch_height.clamp(min, max);
            l.watch = Rect::new(pad, y, inner, wh);
            y += wh;
        }
        l.bar = Rect::new(pad, y, inner, bar_h);
        y += bar_h;
        l.all = Rect::new(pad, y, inner, (l.status.y - y - pad * 0.5).max(header_h));
        self.watch.visible = (((l.watch.h - header_h - row_h) / row_h).floor().max(1.0)) as usize;
        self.all.visible = (((l.all.h - header_h) / row_h).floor().max(1.0)) as usize;
        self.watch.clamp_scroll();
        self.all.clamp_scroll();
        let pw = (w - 4.0 * pad).min(s * 50.0);
        l.palette = Rect::new((w - pw) / 2.0, l.tiles.y + pad * 2.0, pw, 0.0);
        self.layout = l;
    }

    /// Column positions within a table of width `w` (scrollbar excluded).
    fn columns(&self, w: f32) -> Vec<(Col, f32, f32)> {
        let s = self.settings.font_size;
        let cols = &self.settings.columns;
        let flex = if cols.contains(&Col::Path) { Col::Path } else { Col::Name };
        let fixed: f32 = cols.iter().filter(|&&c| c != flex).map(|c| c.width_em() * s).sum();
        let flex_w = (w - fixed).max(flex.width_em() * s * 0.6);
        let mut x = 0.0;
        cols.iter()
            .map(|&c| {
                let cw = if c == flex { flex_w } else { c.width_em() * s };
                let r = (c, x, cw);
                x += cw;
                r
            })
            .collect()
    }

    // ----------------------------------------------------------------- paint

    pub fn paint(&mut self) {
        let Some(mut g) = self.gfx.take() else { return };
        if self.layout.row_h == 0.0 {
            self.gfx = Some(g);
            self.compute_layout();
            g = self.gfx.take().unwrap();
        }
        if g.begin(self.theme.bg) {
            if self.view == View::Main {
                self.paint_tiles(&mut g);
            }
            self.paint_search(&mut g);
            if self.query.is_empty() {
                self.paint_hint(&mut g);
            } else {
                self.paint_table(&mut g, Which::Watch);
            }
            if self.view == View::Main {
                self.paint_bar(&mut g);
                self.paint_table(&mut g, Which::All);
            }
            self.paint_status(&mut g);
            self.paint_overlay(&mut g);
            g.end();
        }
        self.gfx = Some(g);
    }

    /// The text-only summary: one row of label/value cells, no graphs.
    fn paint_compact(&self, g: &mut Gfx) {
        let t = &self.theme;
        let r = self.layout.tiles;
        g.fill_round(r, 6.0, t.panel);
        let Some(snap) = &self.snap else { return };
        let s = &snap.sys;
        let gb = |b: u64| format!("{:.1}", b as f64 / (1024.0 * 1024.0 * 1024.0));
        let used = s.mem_total.saturating_sub(s.mem_avail);
        let mem_pct = if s.mem_total > 0 { used as f32 / s.mem_total as f32 * 100.0 } else { 0.0 };
        let disk = match &s.disk {
            Some(d) => format!("{:.0}%   R {}   W {}", d.active, format::rate(d.read), format::rate(d.write)),
            None => "–".into(),
        };
        let gpu = match (self.settings.gpu, s.gpu) {
            (false, _) => "off".to_string(),
            (true, Some(v)) => format!("{v:.0}%"),
            (true, None) => "–".to_string(),
        };
        let cells: [(&str, String, f32); 6] = [
            ("CPU", format!("{:.0}%", s.cpu), 0.75),
            ("Memory", format!("{mem_pct:.0}%   {} / {} GB", gb(used), gb(s.mem_total)), 1.5),
            ("Disk", disk, 1.55),
            ("Net", format!("↓ {}   ↑ {}", format::rate(s.net.recv), format::rate(s.net.send)), 1.55),
            ("GPU", gpu, 0.7),
            (
                "Procs",
                format!(
                    "{}   {} threads   up {}",
                    format::count(s.procs as u64),
                    format::count(s.threads as u64),
                    format::duration_100ns(s.uptime_ms * 10_000)
                ),
                1.75,
            ),
        ];
        let total: f32 = cells.iter().map(|c| c.2).sum();
        let mut x = r.x;
        for (i, (label, value, w)) in cells.iter().enumerate() {
            let cw = r.w * w / total;
            if i > 0 {
                g.vline(x, r.y + 6.0, r.bottom() - 6.0, t.border);
            }
            let inner = Rect::new(x + 10.0, r.y, cw - 16.0, r.h);
            let lw = g.measure(label, Font::Small) + 8.0;
            g.text(label, Rect::new(inner.x, r.y, lw, r.h), Font::Small, t.text_dim, Align::Left);
            g.text(value, Rect::new(inner.x + lw, r.y, inner.w - lw, r.h), Font::Bold, t.text, Align::Left);
            x += cw;
        }
    }

    fn paint_tiles(&self, g: &mut Gfx) {
        if self.settings.compact_summary {
            return self.paint_compact(g);
        }
        let t = &self.theme;
        let l = &self.layout;
        let r = l.tiles;
        let weights = [1.45f32, 1.25, 1.1, 1.25, 0.9, 1.15];
        let total: f32 = weights.iter().sum();
        let gap = l.pad;
        let avail = r.w - gap * (weights.len() - 1) as f32;
        let mut x = r.x;
        let rects: Vec<Rect> = weights
            .iter()
            .map(|w| {
                let tw = avail * w / total;
                let rr = Rect::new(x, r.y, tw, r.h);
                x += tw + gap;
                rr
            })
            .collect();
        let cap = self.settings.history;
        let Some(snap) = &self.snap else {
            for rr in &rects {
                g.fill_round(*rr, 6.0, t.panel);
            }
            return;
        };
        let s = &snap.sys;
        let gb = |b: u64| format!("{:.1}", b as f64 / (1024.0 * 1024.0 * 1024.0));

        // CPU
        let pct = |v: f32| format!("{v:.0}%");
        // Per-core bars fill the right part of the CPU tile; the graph gets the rest.
        let ncores = s.cores.len().max(1) as f32;
        let bars_w = ((rects[0].w - 20.0) * 0.45).min(ncores * 7.0);
        let reserve = if s.cores.is_empty() { 0.0 } else { bars_w + 14.0 };
        let sub = format!("{} logical processors", s.ncpu);
        self.tile(g, rects[0], "CPU", "", &pct(s.cpu), &sub, &self.graphs.cpu, None, t.cpu, cap, reserve);
        if !s.cores.is_empty() {
            let inner = rects[0].inset(10.0, 8.0);
            let bw = bars_w / ncores;
            let x0 = inner.right() - bars_w;
            for (i, c) in s.cores.iter().enumerate() {
                let bar = Rect::new(x0 + i as f32 * bw, inner.y, (bw - 1.0).max(1.0), inner.h);
                g.fill(bar, alpha(t.cpu, 0.15));
                let fh = bar.h * (c / 100.0);
                g.fill(Rect::new(bar.x, bar.bottom() - fh, bar.w, fh), alpha(t.cpu, 0.85));
            }
        }
        // Memory
        let used = s.mem_total.saturating_sub(s.mem_avail);
        let mem_pct = if s.mem_total > 0 { used as f32 / s.mem_total as f32 * 100.0 } else { 0.0 };
        self.tile(
            g,
            rects[1],
            "Memory",
            &format!("commit {} / {} GB", gb(s.commit), gb(s.commit_limit)),
            &pct(mem_pct),
            &format!("{} of {} GB in use", gb(used), gb(s.mem_total)),
            &self.graphs.mem,
            None,
            t.mem,
            cap,
            0.0,
        );
        // Disk
        let (big, sub) = match &s.disk {
            Some(d) => (pct(d.active), format!("R {}  W {}", format::rate(d.read), format::rate(d.write))),
            None => ("–".into(), "starting…".into()),
        };
        self.tile(g, rects[2], "Disk", "active time", &big, &sub, &self.graphs.disk, None, t.disk, cap, 0.0);
        // Network
        let n = &s.net;
        let label = if n.adapters == 0 {
            "no adapter".to_string()
        } else if n.speed > 0 {
            format!("{} · {}", n.name, format::bits(n.speed as f64))
        } else {
            n.name.clone()
        };
        self.tile(
            g,
            rects[3],
            "Network",
            &label,
            &format!("↓ {}", format::rate(n.recv)),
            &format!("↑ {}", format::rate(n.send)),
            &self.graphs.recv,
            Some(&self.graphs.send),
            t.net_recv,
            cap,
            0.0,
        );
        // GPU
        let (big, sub) = match (self.settings.gpu, s.gpu) {
            (false, _) => ("off".to_string(), "toggle_gpu turns it on".to_string()),
            (true, Some(v)) => (pct(v), "busiest engine".to_string()),
            (true, None) => ("–".to_string(), "starting…".to_string()),
        };
        self.tile(g, rects[4], "GPU", "", &big, &sub, &self.graphs.gpu, None, t.gpu, cap, 0.0);
        // System
        let up = format::duration_100ns(s.uptime_ms * 10_000);
        self.tile(
            g,
            rects[5],
            "Processes",
            &format!("up {up}"),
            &format::count(s.procs as u64),
            &format!("{} threads · {} handles", format::count(s.threads as u64), format::count(s.handles as u64)),
            &VecDeque::new(),
            None,
            t.accent,
            cap,
            0.0,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn tile(
        &self,
        g: &mut Gfx,
        r: Rect,
        label: &str,
        label_right: &str,
        big: &str,
        sub: &str,
        graph: &VecDeque<f32>,
        graph2: Option<&VecDeque<f32>>,
        color: u32,
        cap: usize,
        reserve: f32,
    ) {
        let t = &self.theme;
        g.fill_round(r, 6.0, t.panel);
        let gw = r.w - 2.0 - reserve;
        let gr = Rect::new(r.x + 1.0, r.y + r.h * 0.35, gw, r.h * 0.65 - 1.0);
        g.clip(Rect::new(r.x, r.y, r.w, r.h - 3.0));
        if let Some(g2) = graph2 {
            // Two rate series share one scale.
            let max = graph.iter().chain(g2.iter()).fold(1024.0f32, |a, &b| a.max(b));
            let a: Vec<f32> = graph.iter().map(|v| v / max).collect();
            let b: Vec<f32> = g2.iter().map(|v| v / max).collect();
            g.area(gr, &a, cap, alpha(color, 0.16), alpha(color, 0.8));
            g.area(gr, &b, cap, alpha(t.net_send, 0.12), alpha(t.net_send, 0.8));
        } else if !graph.is_empty() {
            let v: Vec<f32> = graph.iter().copied().collect();
            g.area(gr, &v, cap, alpha(color, 0.16), alpha(color, 0.8));
        }
        g.unclip();
        let inner = r.inset(10.0, 6.0);
        let tw = inner.w - reserve;
        let lh = g.size * 1.35;
        g.text(label, Rect::new(inner.x, inner.y, tw, lh), Font::Small, t.text_dim, Align::Left);
        let lw = g.measure(label, Font::Small) + 8.0;
        g.text(label_right, Rect::new(inner.x + lw, inner.y, tw - lw, lh), Font::Small, t.text_dim, Align::Right);
        let bh = g.size * 2.0;
        g.text(big, Rect::new(inner.x, inner.y + lh, tw, bh), Font::Big, t.text, Align::Left);
        g.text(sub, Rect::new(inner.x, inner.y + lh + bh, tw, lh), Font::Small, t.text_dim, Align::Left);
    }

    fn paint_search(&self, g: &mut Gfx) {
        let t = &self.theme;
        let r = self.layout.search;
        let focused = self.focus == Focus::Search && matches!(self.overlay, Overlay::None);
        let label_w = g.measure("Watch", Font::Bold) + 16.0;
        g.text("Watch", Rect::new(r.x + 2.0, r.y, label_w, r.h), Font::Bold, if focused { t.accent } else { t.text }, Align::Left);
        let bx = Rect::new(r.x + label_w, r.y, r.w - label_w, r.h);
        g.fill_round(bx, 4.0, t.input_bg);
        g.stroke_round(bx, 4.0, if focused { t.accent } else { t.input_border }, if focused { 1.5 } else { 1.0 });
        let count = if self.query.is_empty() {
            String::new()
        } else {
            match self.watch.rows.len() {
                1 => "1 process".to_string(),
                n => format!("{n} processes"),
            }
        };
        let cw = if count.is_empty() { 0.0 } else { g.measure(&count, Font::Small) + 12.0 };
        let tr = Rect::new(bx.x + 8.0, bx.y, bx.w - 16.0 - cw, bx.h);
        g.text(&count, Rect::new(bx.right() - cw - 4.0, bx.y, cw, bx.h), Font::Small, t.text_dim, Align::Right);
        self.paint_textbox(g, &self.search, tr, focused, "excel | code | python !helper    (F1 for search syntax)");
    }

    fn paint_textbox(&self, g: &mut Gfx, tb: &TextBox, r: Rect, focused: bool, placeholder: &str) {
        let t = &self.theme;
        g.clip(r);
        if tb.text.is_empty() {
            g.text(placeholder, r, Font::Normal, t.text_dim, Align::Left);
        } else {
            if tb.has_selection() {
                let (a, b) = tb.selection();
                let xa = g.caret_x(&tb.text, Font::Normal, tb.utf16_pos(a));
                let xb = g.caret_x(&tb.text, Font::Normal, tb.utf16_pos(b));
                let h = g.size * 1.4;
                g.fill(Rect::new(r.x + xa, r.y + (r.h - h) / 2.0, xb - xa, h), t.selection);
            }
            g.text(&tb.text, Rect::new(r.x, r.y, r.w + 1000.0, r.h), Font::Normal, t.text, Align::Left);
        }
        if focused {
            let x = g.caret_x(&tb.text, Font::Normal, tb.utf16_pos(tb.caret));
            let h = g.size * 1.3;
            g.fill(Rect::new(r.x + x, r.y + (r.h - h) / 2.0, 1.5, h), t.accent);
        }
        g.unclip();
    }

    fn paint_hint(&self, g: &mut Gfx) {
        let t = &self.theme;
        let add = self.keymap.short_label(Action::AddToWatch);
        let mut msg = String::from("Type process names above to watch them; | separates names.");
        if !add.is_empty() {
            msg.push_str(&format!(" {add} adds the selected process."));
        }
        g.text(&msg, self.layout.hint.inset(8.0, 0.0), Font::Small, t.text_dim, Align::Left);
    }

    fn paint_bar(&self, g: &mut Gfx) {
        let t = &self.theme;
        let r = self.layout.bar;
        let mid = r.y + r.h / 2.0;
        if !self.query.is_empty() {
            // The bar doubles as the splitter between the tables.
            let grip = Rect::new(r.x + r.w / 2.0 - 16.0, mid - 1.5, 32.0, 3.0);
            g.fill_round(grip, 1.5, t.border);
        }
        let n = self.all.rows.len();
        let dir = if self.all.sort.desc { "▼" } else { "▲" };
        let label = format!("All processes · {n} · sorted by {} {dir}", self.all.sort.col.title());
        let focused = self.focus == Focus::All;
        g.text(&label, Rect::new(r.x + 2.0, r.y, r.w / 2.0 - 24.0, r.h), Font::Small, if focused { t.accent } else { t.text_dim }, Align::Left);
        if let Some(s) = &self.snap {
            let mode = match self.settings.cpu_mode {
                CpuMode::PerCore => "CPU % per core",
                CpuMode::Total => "CPU % of machine",
            };
            let watch = if self.settings.watch_children { " · watching children" } else { "" };
            let info = format!("{mode}{watch} · sample {:.1} ms", s.cost.as_secs_f64() * 1000.0);
            g.text(&info, Rect::new(r.x + r.w / 2.0 + 24.0, r.y, r.w / 2.0 - 26.0, r.h), Font::Small, t.text_dim, Align::Right);
        }
    }

    fn paint_table(&self, g: &mut Gfx, which: Which) {
        let t = &self.theme;
        let l = &self.layout;
        let r = match which {
            Which::Watch => l.watch,
            Which::All => l.all,
        };
        if r.h <= 0.0 {
            return;
        }
        let table = self.table(which);
        let focused = match which {
            Which::Watch => self.focus == Focus::Watch,
            Which::All => self.focus == Focus::All,
        } && matches!(self.overlay, Overlay::None);
        let body_w = r.w - l.sb_w;
        let cols = self.columns(body_w);
        let cell_pad = 6.0;

        // Header
        let hr = Rect::new(r.x, r.y, r.w, l.header_h);
        g.fill(hr, t.panel);
        if focused {
            g.fill(Rect::new(r.x, r.y, r.w, 2.0), t.accent);
        }
        for &(c, x, w) in &cols {
            let mut title = c.title().to_string();
            if c == table.sort.col {
                title = if c.right_aligned() {
                    format!("{} {title}", if table.sort.desc { "▼" } else { "▲" })
                } else {
                    format!("{title} {}", if table.sort.desc { "▼" } else { "▲" })
                };
            }
            let cr = Rect::new(r.x + x + cell_pad, r.y, w - 2.0 * cell_pad, l.header_h);
            let color = if c == table.sort.col { t.text } else { t.text_dim };
            g.text(&title, cr, Font::Bold, color, if c.right_aligned() { Align::Right } else { Align::Left });
            if x > 0.0 {
                g.vline(r.x + x, r.y + 5.0, r.y + l.header_h - 5.0, t.border);
            }
        }
        g.hline(r.x, r.right(), r.y + l.header_h - 1.0, t.border);

        let Some(snap) = &self.snap else { return };
        let body = Rect::new(r.x, r.y + l.header_h, r.w, r.h - l.header_h);
        let totals_h = if which == Which::Watch { l.row_h } else { 0.0 };
        let rows_area = Rect::new(body.x, body.y, body.w, body.h - totals_h);
        if which == Which::Watch && table.rows.is_empty() {
            g.text("No running process matches the search", rows_area, Font::Small, t.text_dim, Align::Center);
        }
        let scale = self.cpu_scale();
        let now = snap.now_100ns;
        g.clip(rows_area);
        for i in 0..=table.visible {
            let idx = table.scroll + i;
            let Some(&pi) = table.rows.get(idx) else { break };
            let p = &snap.procs[pi];
            let y = rows_area.y + i as f32 * l.row_h;
            let rr = Rect::new(r.x, y, body_w, l.row_h);
            let selected = table.sel.is_some() && table.sel_idx == idx;
            if selected {
                g.fill(rr, if focused { t.sel } else { t.sel_inactive });
            } else if self.hover == Some((which, idx)) {
                g.fill(rr, t.row_hover);
            } else if idx % 2 == 1 {
                g.fill(rr, t.row_alt);
            }
            let text_c = if selected && focused { t.sel_text } else { t.text };
            for &(c, x, w) in &cols {
                let cr = Rect::new(r.x + x + cell_pad, y, w - 2.0 * cell_pad, l.row_h);
                if self.settings.heat_bars && matches!(c, Col::Cpu | Col::Gpu) {
                    let v = if c == Col::Cpu { p.cpu * scale } else { p.gpu };
                    let frac = (v / 100.0).clamp(0.0, 1.0);
                    if frac > 0.002 {
                        let bw = (w - 4.0) * frac;
                        g.fill(Rect::new(r.x + x + w - 2.0 - bw, y + 3.0, bw, l.row_h - 6.0), alpha(t.heat, 0.30));
                    }
                }
                let s = c.text(p, scale, now);
                let color = if c.is_zero(p) && !(selected && focused) { t.text_dim } else { text_c };
                g.text(&s, cr, Font::Normal, color, if c.right_aligned() { Align::Right } else { Align::Left });
            }
        }
        g.unclip();

        if which == Which::Watch && !table.rows.is_empty() {
            let y = body.bottom() - totals_h;
            let tr = Rect::new(r.x, y, body_w, totals_h);
            g.fill(tr, t.panel);
            g.hline(r.x, r.x + body_w, y, t.border);
            let ps: Vec<&Proc> = table.rows.iter().map(|&i| &snap.procs[i]).collect();
            for &(c, x, w) in &cols {
                let cr = Rect::new(r.x + x + cell_pad, y, w - 2.0 * cell_pad, totals_h);
                let s = match c {
                    Col::Name => format!("Total ({})", ps.len()),
                    Col::Cpu => format::percent(ps.iter().map(|p| p.cpu).sum::<f32>() * scale),
                    Col::Gpu => format::percent(ps.iter().map(|p| p.gpu).sum::<f32>().min(100.0)),
                    Col::Private => format::bytes(ps.iter().map(|p| p.private).sum()),
                    Col::WorkingSet => format::bytes(ps.iter().map(|p| p.ws).sum()),
                    Col::PrivateWs => format::bytes(ps.iter().map(|p| p.ws_private).sum()),
                    Col::Threads => format::count(ps.iter().map(|p| p.threads as u64).sum()),
                    Col::Handles => format::count(ps.iter().map(|p| p.handles as u64).sum()),
                    Col::IoRead => format::rate(ps.iter().map(|p| p.io_read).sum()),
                    Col::IoWrite => format::rate(ps.iter().map(|p| p.io_write).sum()),
                    Col::IoOther => format::rate(ps.iter().map(|p| p.io_other).sum()),
                    Col::CpuTime => format::duration_100ns(ps.iter().map(|p| p.cpu_time).sum()),
                    _ => String::new(),
                };
                g.text(&s, cr, Font::Bold, t.text, if c.right_aligned() { Align::Right } else { Align::Left });
            }
        }

        // Scrollbar
        if let Some(th) = self.thumb(which) {
            g.fill_round(th, l.sb_w / 2.0 - 1.0, t.scrollbar);
        }
    }

    /// The scroll thumb of a table, when it has more rows than fit.
    fn thumb(&self, which: Which) -> Option<Rect> {
        let l = &self.layout;
        let table = self.table(which);
        let r = match which {
            Which::Watch => l.watch,
            Which::All => l.all,
        };
        let n = table.rows.len();
        if n <= table.visible || r.h <= 0.0 {
            return None;
        }
        let totals_h = if which == Which::Watch { l.row_h } else { 0.0 };
        let track = Rect::new(r.right() - l.sb_w + 2.0, r.y + l.header_h + 2.0, l.sb_w - 4.0, r.h - l.header_h - totals_h - 4.0);
        let th = (track.h * table.visible as f32 / n as f32).max(20.0).min(track.h);
        let max_scroll = (n - table.visible) as f32;
        let y = track.y + (track.h - th) * (table.scroll as f32 / max_scroll);
        Some(Rect::new(track.x, y, track.w, th))
    }

    fn paint_status(&self, g: &mut Gfx) {
        let t = &self.theme;
        let r = self.layout.status;
        g.fill(r, t.panel);
        g.hline(r.x, r.right(), r.y, t.border);
        let mut right = Vec::new();
        if platform::is_elevated() {
            right.push("Administrator".to_string());
        }
        if self.user_paused {
            right.push("⏸ Paused".to_string());
        }
        let views = self.keymap.short_label(Action::NextView);
        right.push(if views.is_empty() { self.view.title().to_string() } else { format!("{} ({views})", self.view.title()) });
        right.push(format!("every {}", fmt_ms(self.settings.refresh_ms)));
        let pal = self.keymap.short_label(Action::Palette);
        if !pal.is_empty() {
            right.push(format!("{pal} commands"));
        }
        let help = self.keymap.short_label(Action::Help);
        if !help.is_empty() {
            right.push(format!("{help} help"));
        }
        let right = right.join("  ·  ");
        let rw = g.measure(&right, Font::Small) + 16.0;
        let inner = r.inset(10.0, 0.0);
        g.text(&right, Rect::new(inner.right() - rw, r.y, rw, r.h), Font::Small, t.text_dim, Align::Right);
        let left_r = Rect::new(inner.x, r.y, inner.w - rw - 10.0, r.h);
        if let Some((msg, err)) = &self.status {
            g.text(msg, left_r, Font::Small, if *err { t.danger } else { t.ok }, Align::Left);
        } else if let Some(p) = self.selected() {
            let mut s = format!("{}  ·  PID {}  ·  parent {}", p.info.name, p.pid, p.ppid);
            if let Some(path) = p.info.path() {
                s.push_str("  ·  ");
                s.push_str(path);
            }
            g.text(&s, left_r, Font::Small, t.text, Align::Left);
        }
    }

    fn paint_overlay(&self, g: &mut Gfx) {
        let t = &self.theme;
        let l = &self.layout;
        match &self.overlay {
            Overlay::None => {}
            Overlay::Palette { input, items, sel, scroll } => {
                g.fill(Rect::new(0.0, 0.0, g.w, g.h), t.overlay);
                let rows = items.len().min(14);
                let mut r = l.palette;
                let ih = (g.size * 2.3).round();
                r.h = ih + rows.max(1) as f32 * l.row_h + 12.0;
                g.fill_round(r, 8.0, t.popup);
                g.stroke_round(r, 8.0, t.border, 1.0);
                let ir = Rect::new(r.x + 8.0, r.y + 6.0, r.w - 16.0, ih - 6.0);
                g.fill_round(ir, 4.0, t.input_bg);
                g.stroke_round(ir, 4.0, t.accent, 1.0);
                self.paint_textbox(g, input, ir.inset(8.0, 0.0), true, "Type a command");
                let y0 = r.y + ih + 4.0;
                if items.is_empty() {
                    g.text("No matching command", Rect::new(r.x + 16.0, y0, r.w - 32.0, l.row_h), Font::Normal, t.text_dim, Align::Left);
                }
                for (i, &bi) in items.iter().enumerate().skip(*scroll).take(rows) {
                    let b = &self.bindings[bi];
                    let y = y0 + (i - scroll) as f32 * l.row_h;
                    let rr = Rect::new(r.x + 6.0, y, r.w - 12.0, l.row_h);
                    if i == *sel {
                        g.fill_round(rr, 4.0, t.sel);
                    }
                    let keys = self.keymap.label(b.action);
                    let kw = if keys.is_empty() { 0.0 } else { g.measure(&keys, Font::Small) + 12.0 };
                    let tc = if i == *sel { t.sel_text } else { t.text };
                    g.text(&b.title, Rect::new(rr.x + 10.0, y, rr.w - kw - 20.0, l.row_h), Font::Normal, tc, Align::Left);
                    g.text(&keys, Rect::new(rr.right() - kw - 8.0, y, kw, l.row_h), Font::Small, if i == *sel { tc } else { t.text_dim }, Align::Right);
                }
            }
            Overlay::Confirm { title, targets } => {
                g.fill(Rect::new(0.0, 0.0, g.w, g.h), t.overlay);
                let shown = targets.len().min(10);
                let extra = targets.len() > shown;
                let lines = shown + extra as usize;
                let w = (g.w - 4.0 * l.pad).min(g.size * 38.0);
                let h = l.row_h * (lines as f32 + 4.0) + 16.0;
                let r = Rect::new((g.w - w) / 2.0, (g.h - h) / 2.5, w, h);
                g.fill_round(r, 8.0, t.popup);
                g.stroke_round(r, 8.0, t.danger, 1.5);
                let x = r.x + 18.0;
                let iw = r.w - 36.0;
                let mut y = r.y + 10.0;
                g.text(title, Rect::new(x, y, iw, l.row_h * 1.3), Font::Bold, t.danger, Align::Left);
                y += l.row_h * 1.5;
                for tg in targets.iter().take(shown) {
                    g.text(&format!("{}   PID {}", tg.name, tg.pid), Rect::new(x + 8.0, y, iw - 8.0, l.row_h), Font::Normal, t.text, Align::Left);
                    y += l.row_h;
                }
                if extra {
                    g.text(&format!("… and {} more", targets.len() - shown), Rect::new(x + 8.0, y, iw, l.row_h), Font::Normal, t.text_dim, Align::Left);
                    y += l.row_h;
                }
                y += l.row_h * 0.5;
                g.text("Enter  end   ·   Esc  cancel", Rect::new(x, y, iw, l.row_h), Font::Small, t.text_dim, Align::Left);
            }
            Overlay::Help { scroll } => {
                g.fill(Rect::new(0.0, 0.0, g.w, g.h), t.overlay);
                let lines = self.help_lines();
                let w = (g.w - 4.0 * l.pad).min(g.size * 60.0);
                let h = (g.h - 4.0 * l.pad).min(l.row_h * (lines.len() as f32 + 2.0));
                let r = Rect::new((g.w - w) / 2.0, (g.h - h) / 2.0, w, h);
                g.fill_round(r, 8.0, t.popup);
                g.stroke_round(r, 8.0, t.border, 1.0);
                let inner = r.inset(20.0, l.row_h);
                g.clip(inner);
                let kw = g.size * 17.0;
                for (i, (a, b)) in lines.iter().enumerate().skip(*scroll) {
                    let y = inner.y + (i - scroll) as f32 * l.row_h;
                    if y > inner.bottom() {
                        break;
                    }
                    if b.is_empty() {
                        g.text(a, Rect::new(inner.x, y, inner.w, l.row_h), Font::Bold, t.accent, Align::Left);
                    } else {
                        g.text(a, Rect::new(inner.x + 8.0, y, kw, l.row_h), Font::Normal, t.text, Align::Left);
                        g.text(b, Rect::new(inner.x + kw + 16.0, y, inner.w - kw - 16.0, l.row_h), Font::Normal, t.text_dim, Align::Left);
                    }
                }
                g.unclip();
            }
        }
    }

    fn help_lines(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = Vec::new();
        let mut add = |a: &str, b: &str| v.push((a.to_string(), b.to_string()));
        add("Watch search", "");
        add("excel", "names containing \"excel\" (case-insensitive)");
        add("micro edge", "every word must appear, in any order");
        add("excel | code", "either one; add as many | alternatives as you like");
        add("code !helper", "leave out names containing \"helper\" (-helper works too)");
        add("=code", "exactly code or code.exe");
        add("^py", "names starting with \"py\"");
        add("pid:1234", "one process ID");
        add("\"two words\"", "a term with a space in it");
        add("Up / Down", "earlier searches (in the search box)");
        add("", "");
        add("Mouse", "");
        add("Click a header", "sort; click again to reverse");
        add("Right-click a row", "process menu");
        add("Drag the bar", "between the tables to resize them");
        add("Ctrl+wheel", "font size");
        add("", "");
        add("Keys (keymap.ini changes them)", "");
        let mut lines = v;
        let mut group = "";
        for b in &self.bindings {
            let keys = self.keymap.label(b.action);
            if keys.is_empty() {
                continue;
            }
            if b.group != group {
                group = b.group;
                lines.push((group.to_string(), String::new()));
            }
            lines.push((keys, b.title.clone()));
        }
        lines.push((String::new(), String::new()));
        lines.push(("Settings folder".into(), String::new()));
        lines.push((ini::config_dir().display().to_string(), "settings.ini · keymap.ini · theme.ini".into()));
        lines
    }

    // ----------------------------------------------------------------- keys

    pub fn on_key(&mut self, vk: u16) -> bool {
        let (ctrl, shift, alt) = mods();
        let key = Key { vk, ctrl, shift, alt };
        let handled = self.key(key);
        if handled {
            self.invalidate();
        }
        handled
    }

    fn key(&mut self, k: Key) -> bool {
        let action = self.keymap.lookup(k);
        // Modal overlays take every key.
        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Confirm { targets, .. } => {
                match k.vk {
                    0x0D | 0x59 => {
                        let targets = std::mem::take(targets);
                        self.overlay = Overlay::None;
                        self.kill_now(targets);
                    }
                    0x1B | 0x4E => self.overlay = Overlay::None,
                    _ => {}
                }
                return true;
            }
            Overlay::Help { scroll } => {
                match k.vk {
                    0x26 => *scroll = scroll.saturating_sub(1),
                    0x28 => *scroll += 1,
                    0x21 => *scroll = scroll.saturating_sub(10),
                    0x22 => *scroll += 10,
                    0x24 => *scroll = 0,
                    _ if action == Some(Action::Help) || matches!(k.vk, 0x1B | 0x0D) => self.overlay = Overlay::None,
                    _ => {}
                }
                let max = self.help_lines().len().saturating_sub(5);
                if let Overlay::Help { scroll } = &mut self.overlay {
                    *scroll = (*scroll).min(max);
                }
                return true;
            }
            Overlay::Palette { .. } => return self.palette_key(k, action),
        }

        if self.focus == Focus::Search {
            if self.search_key(k) {
                return true;
            }
            // Plain typing becomes WM_CHAR text.
            if !k.ctrl && !k.alt && is_printable(k.vk) {
                return false;
            }
        }
        match action {
            Some(a) => {
                self.run(a);
                true
            }
            None => false,
        }
    }

    /// Editing keys of the search box. True when used.
    fn search_key(&mut self, k: Key) -> bool {
        let tb = &mut self.search;
        let mut changed = false;
        match (k.vk, k.ctrl, k.alt) {
            (0x25, _, false) => tb.left(k.ctrl, k.shift),
            (0x27, _, false) => tb.right(k.ctrl, k.shift),
            (0x24, false, false) => tb.home(k.shift),
            (0x23, false, false) => tb.end(k.shift),
            (0x08, _, false) => {
                tb.backspace(k.ctrl);
                changed = true;
            }
            (0x2E, _, false) => {
                tb.delete(k.ctrl);
                changed = true;
            }
            (0x2D, _, false) if !k.ctrl => {}
            (0x41, true, false) if !k.shift => tb.select_all(),
            (0x43, true, false) if !k.shift && tb.has_selection() => {
                let s = tb.selected().to_string();
                platform::copy_text(self.hwnd, &s);
            }
            (0x58, true, false) if !k.shift && tb.has_selection() => {
                let s = tb.cut();
                platform::copy_text(self.hwnd, &s);
                changed = true;
            }
            (0x56, true, false) if !k.shift => {
                if let Some(s) = platform::paste_text(self.hwnd) {
                    self.search.insert(&s);
                    changed = true;
                }
            }
            (0x26, false, false) => {
                // Older search from history.
                let next = self.hist_pos.map(|p| p + 1).unwrap_or(0);
                if next < self.history.len() {
                    if self.hist_pos.is_none() {
                        self.hist_saved = self.search.text.clone();
                    }
                    self.hist_pos = Some(next);
                    let h = self.history[next].clone();
                    self.search.set(&h);
                    changed = true;
                }
            }
            (0x28, false, false) => match self.hist_pos {
                Some(0) => {
                    self.hist_pos = None;
                    let s = self.hist_saved.clone();
                    self.search.set(&s);
                    changed = true;
                }
                Some(p) => {
                    self.hist_pos = Some(p - 1);
                    let h = self.history[p - 1].clone();
                    self.search.set(&h);
                    changed = true;
                }
                None => self.focus_table(Focus::Watch),
            },
            (0x0D, false, false) => {
                self.remember_search();
                self.focus_table(if self.query.is_empty() { Focus::All } else { Focus::Watch });
            }
            (0x1B, false, false) => {
                if self.search.has_selection() {
                    let c = self.search.caret;
                    self.search.anchor = c;
                } else {
                    self.remember_search();
                    self.focus_table(if self.query.is_empty() { Focus::All } else { Focus::Watch });
                }
            }
            _ => return false,
        }
        if changed {
            self.search_changed();
            self.compute_layout();
        }
        true
    }

    pub fn on_char(&mut self, c: u16) {
        if std::mem::take(&mut self.suppress_char) {
            return;
        }
        let Some(ch) = char::from_u32(c as u32) else { return };
        if ch.is_control() {
            return;
        }
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        match &mut self.overlay {
            Overlay::Palette { input, .. } => {
                input.insert(s);
                self.filter_palette();
            }
            Overlay::None if self.focus == Focus::Search => {
                let was_empty = self.query.is_empty();
                self.hist_pos = None;
                self.search.insert(s);
                self.search_changed();
                if was_empty != self.query.is_empty() {
                    self.compute_layout();
                }
            }
            _ => return,
        }
        self.invalidate();
    }

    fn focus_table(&mut self, f: Focus) {
        let f = match (self.view, f) {
            (View::Watch, Focus::All | Focus::Watch) if self.query.is_empty() => Focus::Search,
            (View::Watch, Focus::All) => Focus::Watch,
            (View::Main, Focus::Watch) if self.query.is_empty() => Focus::All,
            _ => f,
        };
        if self.focus == Focus::Search && f != Focus::Search {
            self.remember_search();
        }
        self.focus = f;
        let w = self.active();
        let t = self.table(w);
        if f != Focus::Search && t.sel.is_none() && !t.rows.is_empty() {
            let row = t.sel_idx.min(t.rows.len() - 1);
            self.select_row(w, row);
        }
        self.invalidate();
    }

    fn move_sel(&mut self, delta: isize) {
        let w = self.active();
        let t = self.table(w);
        if t.rows.is_empty() {
            return;
        }
        let last = t.rows.len() as isize - 1;
        let cur = if t.sel.is_some() { t.sel_idx as isize } else if delta > 0 { -1 } else { t.rows.len() as isize };
        let cur = cur.min(last + 1);
        let row = (cur + delta).clamp(0, last) as usize;
        self.select_row(w, row);
    }

    // -------------------------------------------------------------- palette

    fn open_palette(&mut self) {
        self.overlay = Overlay::Palette { input: TextBox::default(), items: Vec::new(), sel: 0, scroll: 0 };
        self.filter_palette();
    }

    fn palette_actions(&self) -> impl Iterator<Item = (usize, &Binding)> {
        self.bindings.iter().enumerate().filter(|(_, b)| {
            !matches!(
                b.action,
                Action::Palette | Action::Up | Action::Down | Action::PageUp | Action::PageDown | Action::Top | Action::Bottom
            )
        })
    }

    fn filter_palette(&mut self) {
        let Overlay::Palette { input, .. } = &self.overlay else { return };
        let q = input.text.clone();
        let mut scored: Vec<(i32, usize)> = self
            .palette_actions()
            .filter_map(|(i, b)| fuzzy(&q, &format!("{} {}", b.title, b.name)).map(|s| (s, i)))
            .collect();
        // Loose (subsequence) matches only when nothing contains the words.
        if scored.iter().any(|(s, _)| *s > 500) {
            scored.retain(|(s, _)| *s > 500);
        }
        // Stable sort keeps the keymap order among equal scores.
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        if let Overlay::Palette { items, sel, scroll, .. } = &mut self.overlay {
            *items = scored.into_iter().map(|(_, i)| i).collect();
            *sel = 0;
            *scroll = 0;
        }
    }

    fn palette_key(&mut self, k: Key, action: Option<Action>) -> bool {
        let Overlay::Palette { input, items, sel, scroll } = &mut self.overlay else { return false };
        let n = items.len();
        let page = 14usize;
        let mut refilter = false;
        match (k.vk, k.ctrl) {
            (0x1B, _) => {
                self.overlay = Overlay::None;
                return true;
            }
            (0x0D, _) => {
                let chosen = items.get(*sel).map(|&i| self.bindings[i].action);
                self.overlay = Overlay::None;
                if let Some(a) = chosen {
                    self.run(a);
                }
                return true;
            }
            (0x26, _) if n > 0 => *sel = (*sel + n - 1) % n,
            (0x28, _) if n > 0 => *sel = (*sel + 1) % n,
            (0x21, _) => *sel = sel.saturating_sub(page),
            (0x22, _) if n > 0 => *sel = (*sel + page).min(n - 1),
            (0x25, _) => input.left(k.ctrl, k.shift),
            (0x27, _) => input.right(k.ctrl, k.shift),
            (0x24, _) => input.home(k.shift),
            (0x23, _) => input.end(k.shift),
            (0x08, _) => {
                input.backspace(k.ctrl);
                refilter = true;
            }
            (0x2E, _) => {
                input.delete(k.ctrl);
                refilter = true;
            }
            (0x41, true) => input.select_all(),
            (0x56, true) => {
                if let Some(s) = platform::paste_text(self.hwnd) {
                    input.insert(&s);
                    refilter = true;
                }
            }
            _ if action == Some(Action::Palette) => {
                self.overlay = Overlay::None;
                return true;
            }
            _ => {
                if !k.ctrl && !k.alt && is_printable(k.vk) {
                    return false;
                }
            }
        }
        if *sel < *scroll {
            *scroll = *sel;
        } else if *sel >= *scroll + page {
            *scroll = *sel + 1 - page;
        }
        if refilter {
            self.filter_palette();
        }
        true
    }

    // -------------------------------------------------------------- actions

    fn run(&mut self, a: Action) {
        use Action::*;
        match a {
            Palette => self.open_palette(),
            Help => self.overlay = Overlay::Help { scroll: 0 },
            Quit => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            FocusSearch => {
                self.focus = Focus::Search;
                self.search.end(false);
            }
            ClearSearch => {
                self.remember_search();
                self.set_search("");
                self.focus = Focus::Search;
                self.compute_layout();
            }
            NextView | PrevView => {
                let i = VIEWS.iter().position(|&v| v == self.view).unwrap_or(0);
                let n = VIEWS.len();
                let next = if a == NextView { VIEWS[(i + 1) % n] } else { VIEWS[(i + n - 1) % n] };
                self.set_view(next);
            }
            NextPane | PrevPane => {
                let order: Vec<Focus> = if self.view == View::Watch {
                    vec![Focus::Search, Focus::Watch]
                } else if self.query.is_empty() {
                    vec![Focus::Search, Focus::All]
                } else {
                    vec![Focus::Search, Focus::Watch, Focus::All]
                };
                let i = order.iter().position(|&f| f == self.focus).unwrap_or(0);
                let n = order.len();
                let next = if a == NextPane { order[(i + 1) % n] } else { order[(i + n - 1) % n] };
                self.focus_table(next);
            }
            FocusWatch => self.focus_table(Focus::Watch),
            FocusAll => {
                self.set_view(View::Main);
                self.focus_table(Focus::All);
            }
            Up => self.move_sel(-1),
            Down => self.move_sel(1),
            PageUp => {
                let v = self.table(self.active()).visible.max(2) as isize - 1;
                self.move_sel(-v);
            }
            PageDown => {
                let v = self.table(self.active()).visible.max(2) as isize - 1;
                self.move_sel(v);
            }
            Top => self.move_sel(-(1 << 30)),
            Bottom => self.move_sel(1 << 30),
            Kill => match self.selected().map(Self::target) {
                Some(t) => self.confirm_kill(format!("End {}?", t.name), vec![t]),
                None => self.flash("Select a process first", true),
            },
            KillTree => match self.selected().map(|p| (p.pid, p.create_time)) {
                Some(root) => {
                    let targets = self.tree_targets(root);
                    let title = format!("End {} and {} child process{}?", targets[0].name, targets.len() - 1, if targets.len() == 2 { "" } else { "es" });
                    self.confirm_kill(title, targets);
                }
                None => self.flash("Select a process first", true),
            },
            KillWatched => {
                let targets: Vec<Target> = match &self.snap {
                    Some(s) => self.watch.rows.iter().map(|&i| Self::target(&s.procs[i])).collect(),
                    None => Vec::new(),
                };
                if targets.is_empty() {
                    self.flash("The watch table is empty", true);
                } else {
                    let n = targets.len();
                    self.confirm_kill(format!("End all {n} watched process{}?", if n == 1 { "" } else { "es" }), targets);
                }
            }
            AddToWatch => self.add_to_watch(),
            CopyRow => {
                if let Some(p) = self.selected() {
                    let s = format!("{}\t{}\t{}", p.info.name, p.pid, p.info.path().unwrap_or(""));
                    platform::copy_text(self.hwnd, &s);
                    self.flash("Copied name, PID and path", false);
                }
            }
            CopyPath => match self.selected().map(|p| p.info.path().map(str::to_string)) {
                Some(Some(path)) => {
                    platform::copy_text(self.hwnd, &path);
                    self.flash(format!("Copied {path}"), false);
                }
                Some(None) => self.flash("Windows does not give the path of this process", true),
                None => {}
            },
            OpenLocation => match self.selected().map(|p| p.info.path().map(str::to_string)) {
                Some(Some(path)) => platform::reveal(&path),
                Some(None) => self.flash("Windows does not give the path of this process", true),
                None => {}
            },
            Sort(c) => self.sort_by(self.active(), c),
            ToggleColumn(c) => {
                let cols = &mut self.settings.columns;
                if let Some(i) = cols.iter().position(|&x| x == c) {
                    cols.remove(i);
                } else {
                    cols.push(c);
                }
                let v = crate::columns::list_to_string(&self.settings.columns);
                self.save_setting("columns", &v);
                self.sync_filter();
            }
            CpuMode => {
                self.settings.cpu_mode = match self.settings.cpu_mode {
                    settings::CpuMode::PerCore => settings::CpuMode::Total,
                    settings::CpuMode::Total => settings::CpuMode::PerCore,
                };
                let v = self.settings.cpu_mode_str();
                self.save_setting("cpu_mode", v);
                self.flash(
                    match self.settings.cpu_mode {
                        settings::CpuMode::PerCore => "CPU %: 100% is one core",
                        settings::CpuMode::Total => "CPU %: 100% is the whole machine",
                    },
                    false,
                );
            }
            ToggleTheme => {
                self.settings.theme = if self.theme.dark { ThemeChoice::Light } else { ThemeChoice::Dark };
                let v = self.settings.theme_str();
                self.save_setting("theme", v);
                self.reload_theme();
            }
            Pause => {
                self.user_paused = !self.user_paused;
                self.update_paused();
            }
            RefreshNow => self.shared.update(|_| {}),
            Faster | Slower => {
                let cur = self.settings.refresh_ms;
                let ms = if a == Faster {
                    REFRESH_STEPS.iter().rev().find(|&&s| s < cur).copied().unwrap_or(REFRESH_STEPS[0])
                } else {
                    REFRESH_STEPS.iter().find(|&&s| s > cur).copied().unwrap_or(*REFRESH_STEPS.last().unwrap())
                };
                self.settings.refresh_ms = ms;
                self.shared.update(|c| c.interval_ms = ms);
                self.save_setting("refresh_ms", &ms.to_string());
                self.flash(format!("Updating every {}", fmt_ms(ms)), false);
            }
            WatchChildren => {
                self.settings.watch_children = !self.settings.watch_children;
                let v = self.settings.watch_children.to_string();
                self.save_setting("watch_children", &v);
                self.sync_filter();
                self.rebuild();
                self.flash(if self.settings.watch_children { "Watching child processes too" } else { "Watching matches only" }, false);
            }
            WatchGrow | WatchShrink => {
                let d = self.layout.row_h * 2.0 * if a == WatchGrow { 1.0 } else { -1.0 };
                self.watch_height = (self.layout.watch.h.max(self.watch_height.min(self.layout.watch.h)) + d).max(40.0);
                self.compute_layout();
                self.watch_height = self.layout.watch.h;
            }
            FontBigger => self.set_font_size(self.settings.font_size + 1.0),
            FontSmaller => self.set_font_size(self.settings.font_size - 1.0),
            FontReset => self.set_font_size(settings::DEFAULT_FONT_SIZE),
            AlwaysOnTop => {
                self.settings.always_on_top = !self.settings.always_on_top;
                platform::set_topmost(self.hwnd, self.settings.always_on_top);
                let v = self.settings.always_on_top.to_string();
                self.save_setting("always_on_top", &v);
            }
            ToggleGpu => {
                self.settings.gpu = !self.settings.gpu;
                let gpu = self.settings.gpu;
                self.sync_filter();
                self.save_setting("gpu", &gpu.to_string());
                self.flash(if gpu { "GPU sampling on" } else { "GPU sampling off" }, false);
            }
            ToggleCompact => {
                self.settings.compact_summary = !self.settings.compact_summary;
                let v = self.settings.compact_summary.to_string();
                self.save_setting("compact_summary", &v);
            }
            ToggleHeat => {
                self.settings.heat_bars = !self.settings.heat_bars;
                let v = self.settings.heat_bars.to_string();
                self.save_setting("heat_bars", &v);
            }
            RestartAdmin => {
                if platform::is_elevated() {
                    self.flash("Already running as administrator", false);
                } else {
                    self.save_state();
                    if platform::restart_elevated() {
                        unsafe {
                            let _ = DestroyWindow(self.hwnd);
                        }
                    }
                }
            }
            WriteKeymap => self.write_and_open(keymap::FILE, self.keymap.to_ini()),
            WriteSettings => self.write_and_open(settings::FILE, self.settings.to_ini()),
            WriteTheme => self.write_and_open(theme::FILE, theme::to_ini()),
            OpenConfigFolder => {
                let d = ini::config_dir();
                let _ = std::fs::create_dir_all(&d);
                platform::open_folder(&d);
            }
            ReloadConfig => self.reload_config(false),
            Priority(level) => {
                if let Some(t) = self.selected().map(Self::target) {
                    match platform::set_priority(&t, level) {
                        Ok(()) => {
                            self.flash(format!("{} priority: {}", t.name, keymap::PRIORITIES[level as usize]), false);
                            self.shared.update(|_| {});
                        }
                        Err(e) => self.flash(format!("Could not change {}: {e}", t.name), true),
                    }
                }
            }
        }
        self.compute_layout();
        self.invalidate();
    }

    fn write_and_open(&mut self, name: &str, text: String) {
        let path = ini::config_path(name);
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        match std::fs::write(&path, text) {
            Ok(()) => {
                self.mtimes = config_mtimes();
                platform::open_file(&path);
                self.flash(format!("Wrote {}", path.display()), false);
            }
            Err(e) => self.flash(format!("Could not write {}: {e}", path.display()), true),
        }
    }

    fn sort_by(&mut self, w: Which, c: Col) {
        let t = self.table_mut(w);
        if t.sort.col == c {
            t.sort.desc = !t.sort.desc;
        } else {
            t.sort = SortState { col: c, desc: c.default_descending() };
        }
        self.rebuild();
        let t = self.table_mut(w);
        if t.sel.is_some() {
            t.reveal();
        }
    }

    fn add_to_watch(&mut self) {
        let Some(p) = self.selected() else {
            return self.flash("Select a process first", true);
        };
        let name = p.info.name_lower.trim_end_matches(".exe").to_string();
        if !self.query.is_empty() && self.query.matches(&p.info.name_lower, p.pid) {
            return self.flash(format!("{} is already watched", p.info.name), false);
        }
        let term = if name.contains(' ') { format!("\"{name}\"") } else { name };
        let text = self.search.text.trim_end().to_string();
        let new = if text.is_empty() { term } else { format!("{text} | {term}") };
        self.set_search(&new);
        self.remember_search();
        self.compute_layout();
        self.flash(format!("Watching: {new}"), false);
    }

    /// The process and everything it started, children first.
    fn tree_targets(&self, root: (u32, u64)) -> Vec<Target> {
        let Some(snap) = &self.snap else { return Vec::new() };
        let procs = &snap.procs;
        let Some(ri) = procs.iter().position(|p| (p.pid, p.create_time) == root) else { return Vec::new() };
        let mut order = vec![ri];
        let mut i = 0;
        while i < order.len() {
            let parent = &procs[order[i]];
            for (ci, c) in procs.iter().enumerate() {
                if c.ppid == parent.pid && c.create_time >= parent.create_time && ci != order[i] && !order.contains(&ci) {
                    order.push(ci);
                }
            }
            i += 1;
        }
        let mut targets: Vec<Target> = order.iter().map(|&i| Self::target(&procs[i])).collect();
        // Root first in the prompt; killed last.
        targets[1..].reverse();
        targets
    }

    fn confirm_kill(&mut self, title: String, targets: Vec<Target>) {
        if targets.is_empty() {
            return;
        }
        if self.settings.confirm_kill {
            self.overlay = Overlay::Confirm { title, targets };
        } else {
            self.kill_now(targets);
        }
    }

    fn kill_now(&mut self, targets: Vec<Target>) {
        let mut ok = 0;
        let mut errors = Vec::new();
        // Children before parents: the list starts with the root, so go in reverse.
        for t in targets.iter().rev() {
            match platform::kill(t) {
                Ok(()) => ok += 1,
                Err(e) => errors.push(format!("{} ({}): {e}", t.name, t.pid)),
            }
        }
        match (ok, errors.len()) {
            (_, 0) if ok == 1 => self.flash(format!("Ended {}", targets[0].name), false),
            (_, 0) => self.flash(format!("Ended {ok} processes"), false),
            (0, 1) => self.flash(format!("Could not end {}", errors[0]), true),
            _ => self.flash(format!("Ended {ok}; could not end {}: {}", errors.len(), errors[0]), true),
        }
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_REFRESH, 250, None);
        }
    }

    // ---------------------------------------------------------------- mouse

    fn dip(&self, x: i32, y: i32) -> (f32, f32) {
        (x as f32 * 96.0 / self.dpi, y as f32 * 96.0 / self.dpi)
    }

    /// The table and row under a point.
    fn row_at(&self, x: f32, y: f32) -> Option<(Which, Option<usize>)> {
        let l = &self.layout;
        for (w, r) in [(Which::Watch, l.watch), (Which::All, l.all)] {
            if !r.contains(x, y) {
                continue;
            }
            if y < r.y + l.header_h {
                return Some((w, None));
            }
            let totals = if w == Which::Watch { l.row_h } else { 0.0 };
            if y >= r.bottom() - totals || x >= r.right() - l.sb_w {
                return Some((w, None));
            }
            let i = ((y - r.y - l.header_h) / l.row_h) as usize;
            let row = self.table(w).scroll + i;
            return Some((w, (row < self.table(w).rows.len()).then_some(row)));
        }
        None
    }

    pub fn on_mouse_down(&mut self, px: i32, py: i32) {
        let (x, y) = self.dip(px, py);
        let l = self.layout;
        match &self.overlay {
            Overlay::None => {}
            Overlay::Palette { items, scroll, .. } => {
                let ih = (self.layout.row_h * 0.0) + (self.gfx.as_ref().map(|g| g.size).unwrap_or(13.0) * 2.3).round();
                let r = l.palette;
                let rows = items.len().min(14);
                let h = ih + rows.max(1) as f32 * l.row_h + 12.0;
                if Rect::new(r.x, r.y, r.w, h).contains(x, y) {
                    let y0 = r.y + ih + 4.0;
                    if y >= y0 {
                        let i = scroll + ((y - y0) / l.row_h) as usize;
                        if let Some(&bi) = items.get(i) {
                            let a = self.bindings[bi].action;
                            self.overlay = Overlay::None;
                            self.run(a);
                        }
                    }
                } else {
                    self.overlay = Overlay::None;
                }
                self.invalidate();
                return;
            }
            Overlay::Help { .. } => {
                self.overlay = Overlay::None;
                self.invalidate();
                return;
            }
            Overlay::Confirm { .. } => return,
        }
        if l.search.contains(x, y) {
            self.focus = Focus::Search;
            if let Some(g) = &mut self.gfx {
                let label_w = g.measure("Watch", Font::Bold) + 16.0;
                let tx = x - (l.search.x + label_w + 8.0);
                let pos = g.hit(&self.search.text, Font::Normal, tx.max(0.0));
                let b = self.search.byte_pos(pos);
                self.search.caret = b;
                self.search.anchor = b;
            }
            self.invalidate();
            return;
        }
        if !self.query.is_empty() && l.bar.contains(x, y) {
            self.drag = Drag::Splitter { start_y: y, start_h: l.watch.h };
            unsafe { SetCapture(self.hwnd) };
            return;
        }
        for w in [Which::Watch, Which::All] {
            if let Some(th) = self.thumb(w) {
                let r = if w == Which::Watch { l.watch } else { l.all };
                if x >= r.right() - l.sb_w && r.contains(x, y) && y > r.y + l.header_h {
                    if th.contains(x, y) {
                        self.drag = Drag::Thumb { which: w, grab: y - th.y };
                        unsafe { SetCapture(self.hwnd) };
                    } else {
                        let t = self.table_mut(w);
                        let page = t.visible.max(1);
                        t.scroll = if y < th.y { t.scroll.saturating_sub(page) } else { t.scroll + page };
                        t.clamp_scroll();
                    }
                    self.invalidate();
                    return;
                }
            }
        }
        if let Some((w, row)) = self.row_at(x, y) {
            self.focus = if w == Which::Watch { Focus::Watch } else { Focus::All };
            match row {
                Some(row) => self.select_row(w, row),
                None if y < (if w == Which::Watch { l.watch.y } else { l.all.y }) + l.header_h => {
                    let r = if w == Which::Watch { l.watch } else { l.all };
                    let cols = self.columns(r.w - l.sb_w);
                    if let Some(&(c, _, _)) = cols.iter().find(|(_, cx, cw)| x >= r.x + cx && x < r.x + cx + cw) {
                        self.sort_by(w, c);
                    }
                }
                None => {}
            }
            self.invalidate();
        }
    }

    pub fn on_mouse_move(&mut self, px: i32, py: i32) {
        let (x, y) = self.dip(px, py);
        match self.drag {
            Drag::Splitter { start_y, start_h } => {
                self.watch_height = (start_h + y - start_y).max(40.0);
                self.compute_layout();
                self.invalidate();
                return;
            }
            Drag::Thumb { which, grab } => {
                let l = self.layout;
                let r = if which == Which::Watch { l.watch } else { l.all };
                let totals = if which == Which::Watch { l.row_h } else { 0.0 };
                let track_y = r.y + l.header_h + 2.0;
                let track_h = r.h - l.header_h - totals - 4.0;
                let t = self.table(which);
                let n = t.rows.len();
                let th = (track_h * t.visible as f32 / n.max(1) as f32).max(20.0).min(track_h);
                let frac = ((y - grab - track_y) / (track_h - th).max(1.0)).clamp(0.0, 1.0);
                let t = self.table_mut(which);
                t.scroll = (frac * n.saturating_sub(t.visible) as f32).round() as usize;
                t.clamp_scroll();
                self.invalidate();
                return;
            }
            Drag::None => {}
        }
        if !self.tracking_mouse {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            self.tracking_mouse = true;
        }
        let hover = match self.row_at(x, y) {
            Some((w, Some(r))) => Some((w, r)),
            _ => None,
        };
        if hover != self.hover {
            self.hover = hover;
            self.invalidate();
        }
    }

    pub fn on_mouse_leave(&mut self) {
        self.tracking_mouse = false;
        if self.hover.take().is_some() {
            self.invalidate();
        }
    }

    pub fn on_mouse_up(&mut self) {
        if !matches!(self.drag, Drag::None) {
            self.drag = Drag::None;
            unsafe {
                let _ = ReleaseCapture();
            }
        }
    }

    /// Lines to scroll; Ctrl+wheel changes the font size.
    pub fn on_wheel(&mut self, delta: i16, px: i32, py: i32) {
        let (ctrl, _, _) = mods();
        if ctrl {
            let d = if delta > 0 { 1.0 } else { -1.0 };
            self.set_font_size(self.settings.font_size + d);
            self.compute_layout();
            self.invalidate();
            return;
        }
        let mut lines = 3u32;
        unsafe {
            let _ = SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut lines as *mut u32 as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        }
        let n = ((delta as i32 * lines.min(20) as i32) / 120) as isize;
        match &mut self.overlay {
            Overlay::Help { scroll } => {
                *scroll = (*scroll as isize - n).max(0) as usize;
                self.invalidate();
                return;
            }
            Overlay::Palette { .. } | Overlay::Confirm { .. } => return,
            Overlay::None => {}
        }
        // Screen coordinates: convert to client.
        let mut pt = POINT { x: px, y: py };
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut pt);
        }
        let (x, y) = self.dip(pt.x, pt.y);
        let w = match self.row_at(x, y) {
            Some((w, _)) => w,
            None if self.layout.all.y <= y => Which::All,
            None => return,
        };
        let t = self.table_mut(w);
        t.scroll = (t.scroll as isize - n).max(0) as usize;
        t.clamp_scroll();
        self.invalidate();
    }

    pub fn on_right_click(&mut self, px: i32, py: i32) {
        let (x, y) = self.dip(px, py);
        if !matches!(self.overlay, Overlay::None) {
            return;
        }
        let Some((w, Some(row))) = self.row_at(x, y) else { return };
        self.focus = if w == Which::Watch { Focus::Watch } else { Focus::All };
        self.select_row(w, row);
        self.invalidate();
        let mut items: Vec<Option<(Action, String)>> = vec![
            Some((Action::Kill, "End process".into())),
            Some((Action::KillTree, "End process tree".into())),
            None,
            Some((Action::AddToWatch, "Add to watch".into())),
            Some((Action::OpenLocation, "Open file location".into())),
            Some((Action::CopyRow, "Copy name, PID and path".into())),
            Some((Action::CopyPath, "Copy path".into())),
            None,
        ];
        for (i, p) in keymap::PRIORITIES.iter().enumerate() {
            items.push(Some((Action::Priority(i as u8), format!("Priority: {p}"))));
        }
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            for (i, it) in items.iter().enumerate() {
                match it {
                    Some((a, label)) => {
                        let keys = self.keymap.short_label(*a);
                        let text = if keys.is_empty() { label.clone() } else { format!("{label}\t{keys}") };
                        let _ = AppendMenuW(menu, MF_STRING, i + 1, &HSTRING::from(text));
                    }
                    None => {
                        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                    }
                }
            }
            let mut pt = POINT { x: px, y: py };
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut pt);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, self.hwnd, None);
            let _ = DestroyMenu(menu);
            if cmd.0 > 0
                && let Some(Some((a, _))) = items.get(cmd.0 as usize - 1)
            {
                self.run(*a);
            }
        }
    }

    /// The cursor for a client point (pixels): a resize arrow over the splitter.
    pub fn cursor_at(&self, px: i32, py: i32) -> Cursor {
        let (x, y) = self.dip(px, py);
        if !matches!(self.overlay, Overlay::None) {
            return Cursor::Arrow;
        }
        if matches!(self.drag, Drag::Splitter { .. }) || (!self.query.is_empty() && self.layout.bar.contains(x, y)) {
            return Cursor::SizeNs;
        }
        if self.layout.search.contains(x, y) {
            return Cursor::Beam;
        }
        Cursor::Arrow
    }
}

pub enum Cursor {
    Arrow,
    SizeNs,
    Beam,
}

fn is_printable(vk: u16) -> bool {
    matches!(vk, 0x20 | 0x30..=0x39 | 0x41..=0x5A | 0x60..=0x6F | 0xBA..=0xC0 | 0xDB..=0xDF | 0xE2)
}

fn fmt_ms(ms: u32) -> String {
    if ms < 1000 { format!("{ms} ms") } else { format!("{:.1} s", ms as f64 / 1000.0).replace(".0 s", " s") }
}
