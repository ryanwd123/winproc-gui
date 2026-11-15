//! The background thread that samples processes and system totals.
//!
//! One `NtQuerySystemInformation(SystemProcessInformation)` call returns every
//! process with its times, memory, handle and I/O counters, so a sample costs
//! about a millisecond. GPU and disk come from PDH counters, network from
//! `GetIfTable2`. Each finished snapshot is handed to the UI thread with a
//! posted message.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessInformation, SystemProcessorPerformanceInformation,
};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND, LPARAM, WPARAM};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::System::Performance::*;
use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows::Win32::System::SystemInformation::{
    GetSystemInfo, GetSystemTimeAsFileTime, GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
    SYSTEM_INFO,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::{PCWSTR, PWSTR, w};

use crate::search::{self, Query};

/// What does not change while a process runs. Shared between snapshots.
pub struct ProcInfo {
    pub name: Box<str>,
    pub name_lower: Box<str>,
    /// Looked up the first time the process is shown, so hidden processes cost nothing.
    path: OnceLock<Option<Box<str>>>,
}

impl ProcInfo {
    pub fn path(&self) -> Option<&str> {
        self.path.get().and_then(|p| p.as_deref())
    }
}

/// Limits a sample to the watched processes (the watch-only view).
#[derive(Clone, PartialEq)]
pub struct Filter {
    pub query: Query,
    pub children: bool,
}

#[derive(Clone)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// FILETIME units (100 ns since 1601). With the PID it names one process.
    pub create_time: u64,
    pub info: Arc<ProcInfo>,
    /// Percent of one core.
    pub cpu: f32,
    /// Total CPU time, 100 ns units.
    pub cpu_time: u64,
    pub private: u64,
    pub ws: u64,
    pub ws_private: u64,
    pub threads: u32,
    pub handles: u32,
    pub gpu: f32,
    pub io_read: f64,
    pub io_write: f64,
    pub io_other: f64,
    pub session: u32,
    pub priority: i32,
}

#[derive(Clone, Default)]
pub struct Disk {
    pub active: f32,
    pub read: f64,
    pub write: f64,
}

#[derive(Clone, Default)]
pub struct Net {
    pub name: String,
    pub send: f64,
    pub recv: f64,
    /// Link speed in bits per second of the busiest adapter.
    pub speed: u64,
    pub adapters: u32,
}

#[derive(Clone, Default)]
pub struct Sys {
    pub ncpu: u32,
    /// Percent of the whole machine.
    pub cpu: f32,
    pub cores: Vec<f32>,
    pub mem_total: u64,
    pub mem_avail: u64,
    pub commit: u64,
    pub commit_limit: u64,
    pub disk: Option<Disk>,
    pub net: Net,
    pub gpu: Option<f32>,
    pub procs: u32,
    pub threads: u32,
    pub handles: u32,
    pub uptime_ms: u64,
}

pub struct Snapshot {
    pub procs: Vec<Proc>,
    pub sys: Sys,
    /// Current time as a FILETIME, for "started" ages.
    pub now_100ns: u64,
    /// False for the first sample, which has no previous one to compute rates from.
    pub rates_valid: bool,
    /// How long the sample took.
    pub cost: Duration,
    /// True when only the watched processes were included.
    pub filtered: bool,
    /// A quick rescan for a new search or view: rows are fresh, but CPU and I/O
    /// rates and the system totals are the last regular sample's. Not a graph point.
    pub peek: bool,
}

/// Settings the UI thread changes while the sampler runs.
pub struct Control {
    pub interval_ms: u32,
    pub paused: bool,
    pub gpu: bool,
    pub quit: bool,
    pub wake: bool,
    /// Only build rows for these processes; None for all of them.
    pub filter: Option<Filter>,
    /// Rescan the rows now without moving the regular sampling schedule.
    pub peek: bool,
}

pub struct Shared {
    pub ctl: Mutex<Control>,
    pub cv: Condvar,
    pub latest: Mutex<Option<Snapshot>>,
}

impl Shared {
    /// Applies `f` to the control block and wakes the sampler so it takes effect now.
    pub fn update(&self, f: impl FnOnce(&mut Control)) {
        let mut c = self.ctl.lock().unwrap();
        f(&mut c);
        c.wake = true;
        self.cv.notify_one();
    }

    /// Changes which processes get rows, and rescans right away for them.
    pub fn set_filter(&self, filter: Option<Filter>) {
        let mut c = self.ctl.lock().unwrap();
        c.filter = filter;
        c.peek = true;
        self.cv.notify_one();
    }

    pub fn take(&self) -> Option<Snapshot> {
        self.latest.lock().unwrap().take()
    }
}

/// Starts the sampler thread. Each snapshot posts `msg` to `hwnd`.
pub fn start(hwnd: HWND, msg: u32, ctl: Control) -> Arc<Shared> {
    let shared = Arc::new(Shared {
        ctl: Mutex::new(ctl),
        cv: Condvar::new(),
        latest: Mutex::new(None),
    });
    let s = shared.clone();
    let hwnd = hwnd.0 as isize;
    std::thread::Builder::new()
        .name("sampler".into())
        .stack_size(256 * 1024)
        .spawn(move || run(s, hwnd, msg))
        .expect("sampler thread");
    shared
}

fn run(shared: Arc<Shared>, hwnd: isize, msg: u32) {
    let mut sampler = Sampler::new();
    let mut count = 0u32;
    let mut due = Instant::now();
    let post = |snap: Snapshot| {
        *shared.latest.lock().unwrap() = Some(snap);
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as _)), msg, WPARAM(0), LPARAM(0));
        }
    };
    loop {
        let (paused, gpu, interval, filter, wake, peek) = {
            let mut c = shared.ctl.lock().unwrap();
            if c.quit {
                return;
            }
            let r = (c.paused, c.gpu, c.interval_ms, c.filter.clone(), c.wake, c.peek);
            c.wake = false;
            c.peek = false;
            r
        };
        let now = Instant::now();
        if !paused {
            if wake || now >= due {
                post(sampler.sample(gpu, filter.as_ref()));
                count += 1;
                // The second sample comes quickly so CPU percentages show up right away.
                let wait = if count == 1 { 250 } else { interval.max(100) };
                due = now + Duration::from_millis(wait as u64);
            } else if peek {
                post(sampler.peek(filter.as_ref()));
            }
        } else {
            due = now;
        }
        let c = shared.ctl.lock().unwrap();
        let timeout = if paused { Duration::from_secs(3600) } else { due.saturating_duration_since(Instant::now()) };
        let (c, _) = shared
            .cv
            .wait_timeout_while(c, timeout, |c| !c.wake && !c.peek && !c.quit)
            .unwrap();
        if c.quit {
            return;
        }
    }
}

/// The fields of `SYSTEM_PROCESS_INFORMATION` (winternl.h only names a few of them).
#[repr(C)]
struct RawProc {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name_len: u16,
    image_name_max: u16,
    image_name_buf: *const u16,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    read_operation_count: i64,
    write_operation_count: i64,
    other_operation_count: i64,
    read_transfer_count: i64,
    write_transfer_count: i64,
    other_transfer_count: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RawCpu {
    idle: i64,
    kernel: i64,
    user: i64,
    dpc: i64,
    interrupt: i64,
    interrupt_count: u32,
}

/// One process from the first pass over the kernel's list.
struct Light {
    off: usize,
    pid: u32,
    ppid: u32,
    create: u64,
    cpu: f32,
    io: (f64, f64, f64),
    info: Arc<ProcInfo>,
}

struct Entry {
    info: Arc<ProcInfo>,
    cpu_time: u64,
    read: u64,
    write: u64,
    other: u64,
    seen: u32,
    /// Rates from the last regular sample, reused by peeks.
    cpu: f32,
    io: (f64, f64, f64),
}

const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC0000004u32 as i32;

struct Sampler {
    buf: Vec<u64>,
    light: Vec<Light>,
    known: HashMap<(u32, u64), Entry>,
    generation: u32,
    last: Option<Instant>,
    ncpu: u32,
    cpu_prev: Vec<RawCpu>,
    net_prev: HashMap<u64, (u64, u64)>,
    pdh: Option<Pdh>,
    pdh_tried: bool,
    /// The last regular sample's totals and per-process GPU, reused by peeks.
    last_sys: Sys,
    last_gpu: HashMap<u32, f32>,
}

impl Sampler {
    fn new() -> Self {
        let mut si = SYSTEM_INFO::default();
        unsafe { GetSystemInfo(&mut si) };
        Sampler {
            buf: vec![0; 64 * 1024],
            light: Vec::new(),
            known: HashMap::new(),
            generation: 0,
            last: None,
            ncpu: si.dwNumberOfProcessors.max(1),
            cpu_prev: Vec::new(),
            net_prev: HashMap::new(),
            pdh: None,
            pdh_tried: false,
            last_sys: Sys::default(),
            last_gpu: HashMap::new(),
        }
    }

    fn sample(&mut self, gpu: bool, filter: Option<&Filter>) -> Snapshot {
        let t0 = Instant::now();
        let dt = self.last.map(|l| t0.duration_since(l).as_secs_f64()).unwrap_or(0.0);
        let rates_valid = dt > 0.0;
        self.last = Some(t0);

        // PDH loads a few DLLs; set it up after the first sample so startup stays fast.
        if self.pdh_tried {
            if self.pdh.is_none() {
                self.pdh = Pdh::new();
            }
        } else {
            self.pdh_tried = true;
        }
        let (disk, gpu_sys, gpu_procs) = match &mut self.pdh {
            Some(p) => p.collect(gpu),
            None => (None, None, HashMap::new()),
        };

        let mut sys = Sys {
            ncpu: self.ncpu,
            disk,
            gpu: if gpu { gpu_sys } else { None },
            ..Default::default()
        };
        let procs = self.processes(dt, &gpu_procs, filter, &mut sys, true);
        if filter.is_none() {
            self.cpu(&mut sys);
            self.memory(&mut sys);
            self.network(&mut sys, dt);
        } else {
            // The watch view shows no summary. Forget the old counters so the
            // first summary after switching back has no stale rates.
            self.cpu_prev.clear();
            self.net_prev.clear();
        }
        sys.uptime_ms = unsafe { GetTickCount64() };
        self.last_sys = sys.clone();
        self.last_gpu = gpu_procs;

        let ft = unsafe { GetSystemTimeAsFileTime() };
        Snapshot {
            procs,
            sys,
            now_100ns: ft_u64(ft),
            rates_valid,
            cost: t0.elapsed(),
            filtered: filter.is_some(),
            peek: false,
        }
    }

    /// Rescans the process list for a new filter without touching the counters
    /// that rates are computed from, so the regular samples stay evenly spaced.
    fn peek(&mut self, filter: Option<&Filter>) -> Snapshot {
        let t0 = Instant::now();
        let mut sys = self.last_sys.clone();
        sys.procs = 0;
        sys.threads = 0;
        sys.handles = 0;
        let gpu = std::mem::take(&mut self.last_gpu);
        let procs = self.processes(0.0, &gpu, filter, &mut sys, false);
        self.last_gpu = gpu;
        let ft = unsafe { GetSystemTimeAsFileTime() };
        Snapshot {
            procs,
            sys,
            now_100ns: ft_u64(ft),
            rates_valid: true,
            cost: t0.elapsed(),
            filtered: filter.is_some(),
            peek: true,
        }
    }

    /// Every process (or only the watched ones), with rates since the last sample.
    /// The totals in `sys` always cover every process. With `commit` false (a peek)
    /// the stored counters are left alone and the last rates are reported.
    fn processes(
        &mut self,
        dt: f64,
        gpu: &HashMap<u32, f32>,
        filter: Option<&Filter>,
        sys: &mut Sys,
        commit: bool,
    ) -> Vec<Proc> {
        loop {
            let mut needed = 0u32;
            let bytes = (self.buf.len() * 8) as u32;
            let st = unsafe {
                NtQuerySystemInformation(
                    SystemProcessInformation,
                    self.buf.as_mut_ptr() as _,
                    bytes,
                    &mut needed,
                )
            };
            if st.0 == STATUS_INFO_LENGTH_MISMATCH {
                // Room for processes that start between the two calls.
                let want = (needed as usize + 64 * 1024) / 8;
                self.buf.resize(want.max(self.buf.len() * 2), 0);
                continue;
            }
            if st.0 < 0 {
                return Vec::new();
            }
            break;
        }
        if commit {
            self.generation = self.generation.wrapping_add(1);
        }
        let generation = self.generation;
        let base = self.buf.as_ptr() as *const u8;
        let raw = |off: usize| unsafe { &*(base.add(off) as *const RawProc) };

        // Pass 1, cheap: update every process's counters, so rates stay right
        // whichever processes are shown, and add up the totals.
        self.light.clear();
        let mut off = 0usize;
        loop {
            let p = raw(off);
            let pid = p.unique_process_id as u32;
            let create = p.create_time as u64;
            let cpu_time = (p.user_time + p.kernel_time) as u64;
            let (read, write, other) = (
                p.read_transfer_count as u64,
                p.write_transfer_count as u64,
                p.other_transfer_count as u64,
            );
            sys.procs += 1;
            sys.threads += p.number_of_threads;
            sys.handles += p.handle_count;
            let (mut cpu, mut io) = (0.0f32, (0.0, 0.0, 0.0));
            let info = match self.known.get_mut(&(pid, create)) {
                Some(e) if !commit => {
                    cpu = e.cpu;
                    io = e.io;
                    e.info.clone()
                }
                Some(e) => {
                    if dt > 0.0 {
                        cpu = (cpu_time.saturating_sub(e.cpu_time) as f64 / (dt * 1e7) * 100.0) as f32;
                        io = (
                            read.saturating_sub(e.read) as f64 / dt,
                            write.saturating_sub(e.write) as f64 / dt,
                            other.saturating_sub(e.other) as f64 / dt,
                        );
                    }
                    e.cpu_time = cpu_time;
                    e.read = read;
                    e.write = write;
                    e.other = other;
                    e.seen = generation;
                    e.cpu = cpu;
                    e.io = io;
                    e.info.clone()
                }
                None => {
                    let name = if p.image_name_buf.is_null() || p.image_name_len == 0 {
                        if pid == 0 { "System Idle Process".to_string() } else { "?".to_string() }
                    } else {
                        let s = unsafe {
                            std::slice::from_raw_parts(p.image_name_buf, p.image_name_len as usize / 2)
                        };
                        String::from_utf16_lossy(s)
                    };
                    let info = Arc::new(ProcInfo {
                        name_lower: name.to_lowercase().into(),
                        name: name.into(),
                        path: OnceLock::new(),
                    });
                    // A peek leaves new processes for the next regular sample to count.
                    if commit {
                        self.known.insert(
                            (pid, create),
                            Entry {
                                info: info.clone(),
                                cpu_time,
                                read,
                                write,
                                other,
                                seen: generation,
                                cpu: 0.0,
                                io: (0.0, 0.0, 0.0),
                            },
                        );
                    }
                    info
                }
            };
            self.light.push(Light {
                off,
                pid,
                ppid: p.inherited_from_unique_process_id as u32,
                create,
                cpu,
                io,
                info,
            });
            if p.next_entry_offset == 0 {
                break;
            }
            off += p.next_entry_offset as usize;
        }
        if commit {
            self.known.retain(|_, e| e.seen == generation);
        }

        // Which processes get a full row.
        let keep: Option<Vec<bool>> = filter.map(|f| {
            let mut hit: Vec<bool> =
                self.light.iter().map(|l| f.query.matches(&l.info.name_lower, l.pid)).collect();
            if f.children {
                let tree: Vec<(u32, u32, u64)> = self.light.iter().map(|l| (l.pid, l.ppid, l.create)).collect();
                search::add_children(&tree, &mut hit);
            }
            hit
        });

        // Pass 2: full rows, and executable paths only for processes that are shown.
        let n = keep.as_ref().map(|k| k.iter().filter(|&&b| b).count()).unwrap_or(self.light.len());
        let mut out = Vec::with_capacity(n);
        for (i, l) in self.light.iter().enumerate() {
            if keep.as_ref().is_some_and(|k| !k[i]) {
                continue;
            }
            let p = raw(l.off);
            l.info.path.get_or_init(|| image_path(l.pid));
            out.push(Proc {
                pid: l.pid,
                ppid: l.ppid,
                create_time: l.create,
                info: l.info.clone(),
                cpu: l.cpu,
                cpu_time: (p.user_time + p.kernel_time) as u64,
                private: p.private_page_count as u64,
                ws: p.working_set_size as u64,
                ws_private: p.working_set_private_size as u64,
                threads: p.number_of_threads,
                handles: p.handle_count,
                gpu: gpu.get(&l.pid).copied().unwrap_or(0.0),
                io_read: l.io.0,
                io_write: l.io.1,
                io_other: l.io.2,
                session: p.session_id,
                priority: p.base_priority,
            });
        }
        out
    }

    fn cpu(&mut self, sys: &mut Sys) {
        let n = self.ncpu as usize;
        let mut cur = vec![RawCpu::default(); n];
        let st = unsafe {
            NtQuerySystemInformation(
                SystemProcessorPerformanceInformation,
                cur.as_mut_ptr() as _,
                (n * size_of::<RawCpu>()) as u32,
                std::ptr::null_mut(),
            )
        };
        if st.0 < 0 {
            return;
        }
        if self.cpu_prev.len() == n {
            let (mut busy_all, mut total_all) = (0i64, 0i64);
            sys.cores = cur
                .iter()
                .zip(&self.cpu_prev)
                .map(|(c, p)| {
                    // Kernel time includes idle time.
                    let total = (c.kernel - p.kernel) + (c.user - p.user);
                    let busy = total - (c.idle - p.idle);
                    busy_all += busy;
                    total_all += total;
                    if total > 0 { (busy as f32 / total as f32 * 100.0).clamp(0.0, 100.0) } else { 0.0 }
                })
                .collect();
            if total_all > 0 {
                sys.cpu = (busy_all as f32 / total_all as f32 * 100.0).clamp(0.0, 100.0);
            }
        } else {
            sys.cores = vec![0.0; n];
        }
        self.cpu_prev = cur;
    }

    fn memory(&self, sys: &mut Sys) {
        let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        if unsafe { GlobalMemoryStatusEx(&mut m) }.is_ok() {
            sys.mem_total = m.ullTotalPhys;
            sys.mem_avail = m.ullAvailPhys;
        }
        let mut pi = PERFORMANCE_INFORMATION::default();
        if unsafe { GetPerformanceInfo(&mut pi, size_of::<PERFORMANCE_INFORMATION>() as u32) }.is_ok() {
            sys.commit = (pi.CommitTotal * pi.PageSize) as u64;
            sys.commit_limit = (pi.CommitLimit * pi.PageSize) as u64;
        }
    }

    fn network(&mut self, sys: &mut Sys, dt: f64) {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if unsafe { GetIfTable2(&mut table) }.is_err() || table.is_null() {
            return;
        }
        let rows = unsafe {
            std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize)
        };
        let mut best: Option<(f64, usize)> = None;
        let mut next = HashMap::with_capacity(4);
        for (i, r) in rows.iter().enumerate() {
            let flags = r.InterfaceAndOperStatusFlags._bitfield;
            let hardware = flags & 1 != 0;
            let filter = flags & 2 != 0;
            if !hardware || filter || r.OperStatus != IfOperStatusUp {
                continue;
            }
            let luid = unsafe { r.InterfaceLuid.Value };
            next.insert(luid, (r.InOctets, r.OutOctets));
            sys.net.adapters += 1;
            let mut traffic = 0.0;
            if let Some(&(i0, o0)) = self.net_prev.get(&luid)
                && dt > 0.0
            {
                let rx = r.InOctets.saturating_sub(i0) as f64 / dt;
                let tx = r.OutOctets.saturating_sub(o0) as f64 / dt;
                sys.net.recv += rx;
                sys.net.send += tx;
                traffic = rx + tx;
            }
            if best.is_none_or(|(t, _)| traffic > t) {
                best = Some((traffic, i));
            }
        }
        if let Some((_, i)) = best {
            let r = &rows[i];
            let len = r.Alias.iter().position(|&c| c == 0).unwrap_or(r.Alias.len());
            sys.net.name = String::from_utf16_lossy(&r.Alias[..len]);
            sys.net.speed = r.ReceiveLinkSpeed.max(r.TransmitLinkSpeed);
        }
        self.net_prev = next;
        unsafe { FreeMibTable(table as _) };
    }
}

fn ft_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

/// The full path of a process's executable, when we are allowed to ask.
fn image_path(pid: u32) -> Option<Box<str>> {
    if pid == 0 || pid == 4 {
        return None;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]).into())
    }
}

const PDH_MORE_DATA: u32 = 0x800007D2;
/// PDH_CSTATUS_VALID_DATA (0) and PDH_CSTATUS_NEW_DATA (1) both mean a good value.
const PDH_CSTATUS_NEW: u32 = 1;
const PDH_FMT_NOCAP100: PDH_FMT = PDH_FMT(0x8000);

/// PDH counters for disk activity and GPU engines.
struct Pdh {
    query: PDH_HQUERY,
    disk_idle: PDH_HCOUNTER,
    disk_read: PDH_HCOUNTER,
    disk_write: PDH_HCOUNTER,
    gpu: Option<PDH_HCOUNTER>,
    buf: Vec<u64>,
}

impl Pdh {
    fn new() -> Option<Pdh> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let add = |path: PCWSTR| {
                let mut c = PDH_HCOUNTER::default();
                (PdhAddEnglishCounterW(query, path, 0, &mut c) == 0).then_some(c)
            };
            let disk_idle = add(w!("\\PhysicalDisk(_Total)\\% Idle Time")).unwrap_or_default();
            let disk_read = add(w!("\\PhysicalDisk(_Total)\\Disk Read Bytes/sec")).unwrap_or_default();
            let disk_write = add(w!("\\PhysicalDisk(_Total)\\Disk Write Bytes/sec")).unwrap_or_default();
            let _ = PdhCollectQueryData(query);
            Some(Pdh { query, disk_idle, disk_read, disk_write, gpu: None, buf: Vec::new() })
        }
    }

    fn value(c: PDH_HCOUNTER) -> Option<f64> {
        if c.is_invalid() {
            return None;
        }
        let mut v = PDH_FMT_COUNTERVALUE::default();
        let r = unsafe { PdhGetFormattedCounterValue(c, PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100.0), None, &mut v) };
        (r == 0 && v.CStatus <= PDH_CSTATUS_NEW).then(|| unsafe { v.Anonymous.doubleValue })
    }

    /// Disk totals, the busiest GPU engine, and each process's busiest engine.
    fn collect(&mut self, gpu: bool) -> (Option<Disk>, Option<f32>, HashMap<u32, f32>) {
        unsafe {
            match (gpu, self.gpu) {
                (true, None) => {
                    let mut c = PDH_HCOUNTER::default();
                    if PdhAddEnglishCounterW(self.query, w!("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut c) == 0 {
                        self.gpu = Some(c);
                    }
                }
                (false, Some(c)) => {
                    let _ = PdhRemoveCounter(c);
                    self.gpu = None;
                }
                _ => {}
            }
            let _ = PdhCollectQueryData(self.query);
        }
        let disk = match (Self::value(self.disk_idle), Self::value(self.disk_read), Self::value(self.disk_write)) {
            (Some(idle), Some(read), Some(write)) => Some(Disk {
                active: (100.0 - idle).clamp(0.0, 100.0) as f32,
                read,
                write,
            }),
            _ => None,
        };
        let mut per_proc = HashMap::new();
        let mut sys_gpu = None;
        if let Some(c) = self.gpu {
            sys_gpu = self.gpu_engines(c, &mut per_proc);
        }
        (disk, sys_gpu, per_proc)
    }

    fn gpu_engines(&mut self, c: PDH_HCOUNTER, per_proc: &mut HashMap<u32, f32>) -> Option<f32> {
        let fmt = PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100.0);
        let (mut size, mut count) = (0u32, 0u32);
        let mut tries = 0;
        loop {
            let r = unsafe {
                if self.buf.is_empty() {
                    PdhGetFormattedCounterArrayW(c, fmt, &mut size, &mut count, None)
                } else {
                    size = (self.buf.len() * 8) as u32;
                    PdhGetFormattedCounterArrayW(c, fmt, &mut size, &mut count, Some(self.buf.as_mut_ptr() as _))
                }
            };
            if r == PDH_MORE_DATA && tries < 4 {
                tries += 1;
                self.buf.resize((size as usize).div_ceil(8) + 512, 0);
                continue;
            }
            if r != 0 || self.buf.is_empty() {
                return None;
            }
            break;
        }
        let items = unsafe {
            std::slice::from_raw_parts(self.buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W, count as usize)
        };
        // Engines are keyed by a hash of the name after the PID ("luid_..._eng_3_engtype_3D").
        let mut engines: HashMap<u64, f64> = HashMap::new();
        for it in items {
            if it.FmtValue.CStatus > PDH_CSTATUS_NEW {
                continue;
            }
            let v = unsafe { it.FmtValue.Anonymous.doubleValue };
            let name = unsafe { it.szName.as_wide() };
            let Some(rest) = strip_prefix_w(name, "pid_") else { continue };
            let digits = rest.iter().take_while(|&&c| (b'0' as u16..=b'9' as u16).contains(&c)).count();
            let pid: u32 = rest[..digits].iter().fold(0u32, |a, &c| a.wrapping_mul(10).wrapping_add((c - b'0' as u16) as u32));
            let key = rest[digits..].iter().fold(0xcbf29ce484222325u64, |h, &c| (h ^ c as u64).wrapping_mul(0x100000001b3));
            *engines.entry(key).or_default() += v;
            let e = per_proc.entry(pid).or_insert(0.0);
            *e = e.max(v.min(100.0) as f32);
        }
        // Items keep pointers into the buffer only until the next call.
        let max = engines.values().fold(0.0f64, |a, &b| a.max(b));
        Some(max.min(100.0) as f32)
    }
}

fn strip_prefix_w<'a>(s: &'a [u16], prefix: &str) -> Option<&'a [u16]> {
    let p: Vec<u16> = prefix.encode_utf16().collect();
    s.starts_with(&p).then(|| &s[p.len()..])
}

impl Drop for Pdh {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}
