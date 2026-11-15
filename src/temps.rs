//! Temperature sensors that Windows exposes without a kernel driver:
//!
//! - GPUs, through `D3DKMTQueryAdapterInfo(KMTQAITYPE_ADAPTERPERFDATA)`, the
//!   same call Task Manager uses (also gives fan speed and power).
//! - Drives, through `IOCTL_STORAGE_QUERY_PROPERTY(StorageDeviceTemperatureProperty)`
//!   on a `\\.\PhysicalDriveN` handle opened with no access rights, so it works
//!   without elevation.
//! - ACPI thermal zones, through the PDH `Thermal Zone Information` counters.
//!   On many desktop boards these are fixed values, not real sensors.
//!
//! CPU core temperatures need a driver that reads model-specific registers, so
//! they are not here. Everything is opened once, the first time the view is
//! shown, and read only while it is.

use windows::Wdk::Graphics::Direct3D::*;
use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::*;
use windows::Win32::System::Performance::*;
use windows::core::{HSTRING, PCWSTR, w};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Gpu,
    Drive,
    Zone,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Kind::Gpu => "GPU",
            Kind::Drive => "Drive",
            Kind::Zone => "ACPI zone",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Sensor {
    pub kind: Kind,
    pub name: String,
    /// None when the sensor did not answer this time.
    pub celsius: Option<f32>,
    /// Extra readings, like fan speed and power for a GPU.
    pub detail: String,
    /// The device's own warning and critical temperatures, when it reports them.
    pub warn: Option<f32>,
    pub crit: Option<f32>,
}

struct Gpu {
    handle: u32,
    name: String,
    /// Linked adapters have several physical GPUs behind one handle.
    physical: u32,
}

struct Drive {
    handle: HANDLE,
    name: String,
}

struct Zones {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
    /// The counter is in tenths of a kelvin; the fallback one in kelvin.
    tenths: bool,
    buf: Vec<u64>,
}

pub struct Temps {
    gpus: Vec<Gpu>,
    drives: Vec<Drive>,
    zones: Option<Zones>,
    /// PDH takes about 100 ms to set up, so the zones are opened on the second read.
    reads: u32,
}

impl Temps {
    pub fn new() -> Temps {
        let mut t = Temps { gpus: open_gpus(), drives: open_drives(), zones: None, reads: 0 };
        // Keep only GPUs that report a temperature (integrated ones often do not).
        let mut keep = Vec::new();
        for g in std::mem::take(&mut t.gpus) {
            if (0..g.physical).any(|i| perf_data(g.handle, i).is_some_and(|p| p.Temperature > 0)) {
                keep.push(g);
            } else {
                close_adapter(g.handle);
            }
        }
        t.gpus = keep;
        t
    }

    pub fn read(&mut self) -> Vec<Sensor> {
        let mut out = Vec::new();
        for g in &self.gpus {
            for i in 0..g.physical {
                let p = perf_data(g.handle, i);
                let name = if g.physical > 1 { format!("{} #{}", g.name, i + 1) } else { g.name.clone() };
                let mut detail = Vec::new();
                if let Some(p) = &p {
                    if p.FanRPM > 0 {
                        detail.push(format!("fan {} rpm", p.FanRPM));
                    }
                    if p.Power > 0 {
                        // Tenths of a percent of the board's power limit.
                        detail.push(format!("power {:.0}%", p.Power as f32 / 10.0));
                    }
                }
                out.push(Sensor {
                    kind: Kind::Gpu,
                    name,
                    celsius: p.filter(|p| p.Temperature > 0).map(|p| p.Temperature as f32 / 10.0),
                    detail: detail.join("  ·  "),
                    warn: None,
                    crit: None,
                });
            }
        }
        for d in &self.drives {
            let (celsius, warn, crit) = match drive_temp(d.handle) {
                Some((t, w, c)) => (Some(t), w, c),
                None => (None, None, None),
            };
            out.push(Sensor { kind: Kind::Drive, name: d.name.clone(), celsius, detail: String::new(), warn, crit });
        }
        if self.reads == 1 {
            self.zones = Zones::open();
        }
        self.reads = self.reads.saturating_add(1);
        if let Some(z) = &mut self.zones {
            out.extend(z.read());
        }
        out
    }
}

impl Drop for Temps {
    fn drop(&mut self) {
        for g in &self.gpus {
            close_adapter(g.handle);
        }
        for d in &self.drives {
            unsafe {
                let _ = CloseHandle(d.handle);
            }
        }
    }
}

// ------------------------------------------------------------------- GPUs

fn open_gpus() -> Vec<Gpu> {
    let mut e = D3DKMT_ENUMADAPTERS2::default();
    if unsafe { D3DKMTEnumAdapters2(&mut e) }.is_err() || e.NumAdapters == 0 {
        return Vec::new();
    }
    let mut list = vec![D3DKMT_ADAPTERINFO::default(); e.NumAdapters as usize];
    e.pAdapters = list.as_mut_ptr();
    if unsafe { D3DKMTEnumAdapters2(&mut e) }.is_err() {
        return Vec::new();
    }
    list.truncate(e.NumAdapters as usize);
    let mut seen: Vec<LUID> = Vec::new();
    let mut out = Vec::new();
    for a in list {
        let luid = a.AdapterLuid;
        if seen.iter().any(|l| l.LowPart == luid.LowPart && l.HighPart == luid.HighPart) {
            close_adapter(a.hAdapter);
            continue;
        }
        seen.push(luid);
        let mut reg = D3DKMT_ADAPTERREGISTRYINFO::default();
        let name = if query(a.hAdapter, KMTQAITYPE_ADAPTERREGISTRYINFO, &mut reg) {
            wide(&reg.AdapterString)
        } else {
            String::new()
        };
        let mut physical = 0u32;
        if !query(a.hAdapter, KMTQAITYPE_PHYSICALADAPTERCOUNT, &mut physical) {
            physical = 1;
        }
        out.push(Gpu { handle: a.hAdapter, name: if name.is_empty() { "GPU".into() } else { name }, physical: physical.clamp(1, 8) });
    }
    out
}

fn query<T>(adapter: u32, ty: KMTQUERYADAPTERINFOTYPE, data: &mut T) -> bool {
    let mut q = D3DKMT_QUERYADAPTERINFO {
        hAdapter: adapter,
        Type: ty,
        pPrivateDriverData: data as *mut T as *mut _,
        PrivateDriverDataSize: size_of::<T>() as u32,
    };
    unsafe { D3DKMTQueryAdapterInfo(&mut q) }.is_ok()
}

fn perf_data(adapter: u32, index: u32) -> Option<D3DKMT_ADAPTER_PERFDATA> {
    let mut p = D3DKMT_ADAPTER_PERFDATA { PhysicalAdapterIndex: index, ..Default::default() };
    query(adapter, KMTQAITYPE_ADAPTERPERFDATA, &mut p).then_some(p)
}

fn close_adapter(handle: u32) {
    let c = D3DKMT_CLOSEADAPTER { hAdapter: handle };
    unsafe {
        let _ = D3DKMTCloseAdapter(&c);
    }
}

// ----------------------------------------------------------------- drives

fn open_drives() -> Vec<Drive> {
    let mut out = Vec::new();
    for n in 0..32 {
        let path = HSTRING::from(format!("\\\\.\\PhysicalDrive{n}"));
        // No access rights: enough for property queries, and needs no elevation.
        let Ok(h) = (unsafe {
            CreateFileW(&path, 0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None)
        }) else {
            continue;
        };
        if drive_temp(h).is_none() {
            unsafe {
                let _ = CloseHandle(h);
            }
            continue;
        }
        let name = drive_name(h).unwrap_or_else(|| format!("Drive {n}"));
        out.push(Drive { handle: h, name });
    }
    out
}

fn storage_query(h: HANDLE, id: STORAGE_PROPERTY_ID, out: &mut [u64]) -> bool {
    let q = STORAGE_PROPERTY_QUERY { PropertyId: id, QueryType: PropertyStandardQuery, ..Default::default() };
    let mut got = 0u32;
    unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&q as *const _ as *const _),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(out.as_mut_ptr() as *mut _),
            (out.len() * 8) as u32,
            Some(&mut got),
            None,
        )
    }
    .is_ok()
        && got >= 8
}

fn drive_name(h: HANDLE) -> Option<String> {
    let mut buf = vec![0u64; 128];
    if !storage_query(h, StorageDeviceProperty, &mut buf) {
        return None;
    }
    let d = unsafe { &*(buf.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR) };
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, buf.len() * 8) };
    let field = |off: u32| -> String {
        let off = off as usize;
        if off == 0 || off >= bytes.len() {
            return String::new();
        }
        let end = bytes[off..].iter().position(|&b| b == 0).map_or(bytes.len(), |e| off + e);
        String::from_utf8_lossy(&bytes[off..end]).trim().to_string()
    };
    let vendor = field(d.VendorIdOffset);
    let product = field(d.ProductIdOffset);
    let name = if vendor.is_empty() || product.starts_with(&vendor) { product } else { format!("{vendor} {product}") };
    (!name.is_empty()).then_some(name)
}

/// (current, warning, critical) in °C.
fn drive_temp(h: HANDLE) -> Option<(f32, Option<f32>, Option<f32>)> {
    let mut buf = [0u64; 32];
    if !storage_query(h, StorageDeviceTemperatureProperty, &mut buf) {
        return None;
    }
    let d = unsafe { &*(buf.as_ptr() as *const STORAGE_TEMPERATURE_DATA_DESCRIPTOR) };
    if d.InfoCount == 0 {
        return None;
    }
    // The first entry is the device's overall (composite) temperature.
    let t = d.TemperatureInfo[0].Temperature;
    let limit = |v: i16| (v > 0).then_some(v as f32);
    (t > 0).then(|| (t as f32, limit(d.WarningTemperature), limit(d.CriticalTemperature)))
}

// -------------------------------------------------------------- ACPI zones

impl Zones {
    fn open() -> Option<Zones> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut counter = PDH_HCOUNTER::default();
            let mut tenths = true;
            if PdhAddEnglishCounterW(query, w!("\\Thermal Zone Information(*)\\High Precision Temperature"), 0, &mut counter) != 0 {
                tenths = false;
                if PdhAddEnglishCounterW(query, w!("\\Thermal Zone Information(*)\\Temperature"), 0, &mut counter) != 0 {
                    let _ = PdhCloseQuery(query);
                    return None;
                }
            }
            Some(Zones { query, counter, tenths, buf: Vec::new() })
        }
    }

    fn read(&mut self) -> Vec<Sensor> {
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return Vec::new();
        }
        let fmt = PDH_FMT_DOUBLE;
        let (mut size, mut count) = (0u32, 0u32);
        for _ in 0..4 {
            let r = unsafe {
                if self.buf.is_empty() {
                    PdhGetFormattedCounterArrayW(self.counter, fmt, &mut size, &mut count, None)
                } else {
                    size = (self.buf.len() * 8) as u32;
                    PdhGetFormattedCounterArrayW(self.counter, fmt, &mut size, &mut count, Some(self.buf.as_mut_ptr() as _))
                }
            };
            if r == PDH_MORE_DATA {
                self.buf.resize((size as usize).div_ceil(8) + 64, 0);
                continue;
            }
            if r != 0 || self.buf.is_empty() {
                return Vec::new();
            }
            let items = unsafe {
                std::slice::from_raw_parts(self.buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W, count as usize)
            };
            let mut out: Vec<Sensor> = items
                .iter()
                .filter(|it| it.FmtValue.CStatus <= PDH_CSTATUS_NEW_DATA)
                .map(|it| {
                    let v = unsafe { it.FmtValue.Anonymous.doubleValue };
                    let k = if self.tenths { v / 10.0 } else { v };
                    let raw = unsafe { it.szName.to_string() }.unwrap_or_default();
                    // "\_TZ.TZ00" -> "TZ00"
                    let name = raw.rsplit(['.', '\\']).next().unwrap_or(&raw).to_uppercase();
                    Sensor {
                        kind: Kind::Zone,
                        name: format!("Thermal zone {name}"),
                        celsius: (k > 0.0).then_some((k - 273.15) as f32),
                        detail: String::new(),
                        warn: None,
                        crit: None,
                    }
                })
                .collect();
            out.sort_by(|a, b| a.name.cmp(&b.name));
            return out;
        }
        Vec::new()
    }
}

impl Drop for Zones {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}

fn wide(s: &[u16]) -> String {
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prints what this machine reports: `cargo test --release temps -- --nocapture`.
    #[test]
    fn reads_without_crashing() {
        let t0 = std::time::Instant::now();
        let mut t = Temps::new();
        let open = t0.elapsed();
        t.read();
        let t1 = std::time::Instant::now();
        let v = t.read();
        let read = t1.elapsed();
        for s in &v {
            println!("{:<10} {:<36} {:?} warn {:?} crit {:?}  {}", s.kind.title(), s.name, s.celsius, s.warn, s.crit, s.detail);
        }
        println!("open {open:?}, read {read:?}");
        for s in &v {
            if let Some(c) = s.celsius {
                assert!((-40.0..150.0).contains(&c), "{} reads {c}", s.name);
            }
        }
    }
}

