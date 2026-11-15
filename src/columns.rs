//! The process table's columns: names for the INI files, titles, widths, and how
//! each one formats and sorts a process.

use std::cmp::Ordering;

use crate::format;
use crate::sampler::Proc;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Col {
    Name,
    Pid,
    Ppid,
    Cpu,
    CpuTime,
    Private,
    WorkingSet,
    PrivateWs,
    Threads,
    Handles,
    Gpu,
    IoRead,
    IoWrite,
    IoOther,
    Session,
    Priority,
    Started,
    Path,
}

pub const ALL: [Col; 18] = [
    Col::Name,
    Col::Pid,
    Col::Ppid,
    Col::Cpu,
    Col::CpuTime,
    Col::Private,
    Col::WorkingSet,
    Col::PrivateWs,
    Col::Threads,
    Col::Handles,
    Col::Gpu,
    Col::IoRead,
    Col::IoWrite,
    Col::IoOther,
    Col::Session,
    Col::Priority,
    Col::Started,
    Col::Path,
];

pub const DEFAULT: &[Col] = &[
    Col::Name,
    Col::Pid,
    Col::Cpu,
    Col::Private,
    Col::WorkingSet,
    Col::Threads,
    Col::Handles,
    Col::Gpu,
    Col::IoRead,
    Col::IoWrite,
];

impl Col {
    /// The name used in settings.ini and keymap.ini.
    pub fn key(self) -> &'static str {
        match self {
            Col::Name => "name",
            Col::Pid => "pid",
            Col::Ppid => "ppid",
            Col::Cpu => "cpu",
            Col::CpuTime => "cpu_time",
            Col::Private => "private",
            Col::WorkingSet => "working_set",
            Col::PrivateWs => "private_ws",
            Col::Threads => "threads",
            Col::Handles => "handles",
            Col::Gpu => "gpu",
            Col::IoRead => "io_read",
            Col::IoWrite => "io_write",
            Col::IoOther => "io_other",
            Col::Session => "session",
            Col::Priority => "priority",
            Col::Started => "started",
            Col::Path => "path",
        }
    }

    pub fn from_key(s: &str) -> Option<Col> {
        let s = s.trim().to_ascii_lowercase();
        ALL.iter().copied().find(|c| c.key() == s)
    }

    /// The header text.
    pub fn title(self) -> &'static str {
        match self {
            Col::Name => "Name",
            Col::Pid => "PID",
            Col::Ppid => "Parent",
            Col::Cpu => "CPU %",
            Col::CpuTime => "CPU time",
            Col::Private => "Private",
            Col::WorkingSet => "Working set",
            Col::PrivateWs => "Private WS",
            Col::Threads => "Threads",
            Col::Handles => "Handles",
            Col::Gpu => "GPU %",
            Col::IoRead => "Read/s",
            Col::IoWrite => "Write/s",
            Col::IoOther => "Other I/O/s",
            Col::Session => "Session",
            Col::Priority => "Priority",
            Col::Started => "Started",
            Col::Path => "Path",
        }
    }

    /// A longer description for the command palette and settings.ini.
    pub fn describe(self) -> &'static str {
        match self {
            Col::Name => "process name",
            Col::Pid => "process ID",
            Col::Ppid => "parent process ID",
            Col::Cpu => "CPU usage",
            Col::CpuTime => "total CPU time used",
            Col::Private => "private bytes (committed memory only this process uses)",
            Col::WorkingSet => "working set (physical memory in use, including shared)",
            Col::PrivateWs => "private working set (what Task Manager calls Memory)",
            Col::Threads => "thread count",
            Col::Handles => "handle count",
            Col::Gpu => "GPU usage (busiest engine)",
            Col::IoRead => "I/O bytes read per second",
            Col::IoWrite => "I/O bytes written per second",
            Col::IoOther => "other I/O bytes per second",
            Col::Session => "session ID",
            Col::Priority => "base priority",
            Col::Started => "how long ago the process started",
            Col::Path => "executable path",
        }
    }

    /// Width in ems of the font size. `None` for the column that takes the rest.
    pub fn width_em(self) -> f32 {
        match self {
            Col::Name => 16.0,
            Col::Pid | Col::Ppid => 5.0,
            Col::Cpu | Col::Gpu => 6.0,
            Col::CpuTime => 6.5,
            Col::Private | Col::WorkingSet | Col::PrivateWs => 6.8,
            Col::Threads => 5.0,
            Col::Handles => 5.5,
            Col::IoRead | Col::IoWrite | Col::IoOther => 6.8,
            Col::Session => 5.0,
            Col::Priority => 5.0,
            Col::Started => 5.5,
            Col::Path => 20.0,
        }
    }

    pub fn right_aligned(self) -> bool {
        !matches!(self, Col::Name | Col::Path)
    }

    /// Numbers sort largest first on the first click; text sorts A to Z.
    pub fn default_descending(self) -> bool {
        self.right_aligned() && !matches!(self, Col::Pid | Col::Ppid | Col::Session)
    }

    /// The cell text. `cpu_scale` turns per-core percent into the display mode.
    pub fn text(self, p: &Proc, cpu_scale: f32, now_100ns: u64) -> String {
        match self {
            Col::Name => p.info.name.to_string(),
            Col::Pid => p.pid.to_string(),
            Col::Ppid => p.ppid.to_string(),
            Col::Cpu => format::percent(p.cpu * cpu_scale),
            Col::CpuTime => format::duration_100ns(p.cpu_time),
            Col::Private => format::bytes(p.private),
            Col::WorkingSet => format::bytes(p.ws),
            Col::PrivateWs => format::bytes(p.ws_private),
            Col::Threads => format::count(p.threads as u64),
            Col::Handles => format::count(p.handles as u64),
            Col::Gpu => format::percent(p.gpu),
            Col::IoRead => format::rate(p.io_read),
            Col::IoWrite => format::rate(p.io_write),
            Col::IoOther => format::rate(p.io_other),
            Col::Session => p.session.to_string(),
            Col::Priority => p.priority.to_string(),
            Col::Started => {
                if p.create_time == 0 {
                    String::new()
                } else {
                    format::duration_100ns(now_100ns.saturating_sub(p.create_time))
                }
            }
            Col::Path => p.info.path.as_deref().unwrap_or("").to_string(),
        }
    }

    /// Whether the value is zero, so the cell can be drawn dimmed.
    pub fn is_zero(self, p: &Proc) -> bool {
        match self {
            Col::Cpu => p.cpu < 0.05,
            Col::Gpu => p.gpu < 0.05,
            Col::IoRead => p.io_read < 1.0,
            Col::IoWrite => p.io_write < 1.0,
            Col::IoOther => p.io_other < 1.0,
            _ => false,
        }
    }

    /// Compares two processes by this column, ascending.
    pub fn cmp(self, a: &Proc, b: &Proc) -> Ordering {
        fn f(x: f64, y: f64) -> Ordering {
            x.partial_cmp(&y).unwrap_or(Ordering::Equal)
        }
        match self {
            Col::Name => a.info.name_lower.cmp(&b.info.name_lower),
            Col::Pid => a.pid.cmp(&b.pid),
            Col::Ppid => a.ppid.cmp(&b.ppid),
            Col::Cpu => f(a.cpu as f64, b.cpu as f64),
            Col::CpuTime => a.cpu_time.cmp(&b.cpu_time),
            Col::Private => a.private.cmp(&b.private),
            Col::WorkingSet => a.ws.cmp(&b.ws),
            Col::PrivateWs => a.ws_private.cmp(&b.ws_private),
            Col::Threads => a.threads.cmp(&b.threads),
            Col::Handles => a.handles.cmp(&b.handles),
            Col::Gpu => f(a.gpu as f64, b.gpu as f64),
            Col::IoRead => f(a.io_read, b.io_read),
            Col::IoWrite => f(a.io_write, b.io_write),
            Col::IoOther => f(a.io_other, b.io_other),
            Col::Session => a.session.cmp(&b.session),
            Col::Priority => a.priority.cmp(&b.priority),
            // Newer processes have larger create times; "started" sorts by age.
            Col::Started => b.create_time.cmp(&a.create_time),
            Col::Path => a.info.path.cmp(&b.info.path),
        }
    }
}

/// Parses a comma-separated column list, skipping unknown names. Name always
/// stays, first if it was left out.
pub fn parse_list(s: &str) -> Vec<Col> {
    let mut cols: Vec<Col> = Vec::new();
    for c in s.split(',').filter_map(Col::from_key) {
        if !cols.contains(&c) {
            cols.push(c);
        }
    }
    if !cols.contains(&Col::Name) {
        cols.insert(0, Col::Name);
    }
    cols
}

pub fn list_to_string(cols: &[Col]) -> String {
    cols.iter().map(|c| c.key()).collect::<Vec<_>>().join(", ")
}
