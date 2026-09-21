//! Out-of-process PDH hardware poll (`gameoverlayui.dll` `sub_1800C8BA0`).
//!
//! GPU % (`0x1800CA5AB`–`0x1800CAE50`): `\GPU Engine(*)\Running Time`,
//! `PdhAddEnglishCounterA` + `PdhGetFormattedCounterArrayA` `PDH_FMT_LARGE`.
//! Skip instances without `_luid_`. Require `engtype_`; accept every type.
//! No `pid_` filter. Sum same (LUID, engtype), then max across engtypes.
//! `busy_us = ΔRunningTime / 10`; `wall_us = ΔQPC * (1e6 / freq)`; clamp; `* 100`.
//! Hide GPU % until a second sample exists. NVAPI/ADLX not collected (PDH-only).

use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use windows::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_A, PDH_FMT_DOUBLE,
    PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterA, PdhCloseQuery,
    PdhCollectQueryData, PdhGetFormattedCounterArrayA, PdhOpenQueryA, QueryPerformanceCounter,
    QueryPerformanceFrequency,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::core::{PCSTR, PSTR, s};

const POLL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default)]
pub struct HwSample {
    pub gpu_util: Option<f64>,
    pub cpu_util: Option<f64>,
    pub ram_used_mb: Option<f64>,
    pub ram_total_mb: Option<f64>,
    pub vram_dedicated_mb: Option<f64>,
    pub vram_shared_mb: Option<f64>,
}

struct Slot {
    sample: HwSample,
    luid: Option<(u32, i32)>,
}

static SLOT: OnceLock<Arc<Mutex<Slot>>> = OnceLock::new();

fn slot() -> Arc<Mutex<Slot>> {
    SLOT.get_or_init(|| {
        Arc::new(Mutex::new(Slot {
            sample: HwSample::default(),
            luid: None,
        }))
    })
    .clone()
}

pub fn start() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let slot = slot();
    thread::Builder::new()
        .name("glint-hw-monitor".into())
        .spawn(move || poll_loop(slot))
        .ok();
}

pub fn set_adapter_luid(low: u32, high: i32) {
    if let Ok(mut g) = slot().lock() {
        g.luid = Some((low, high));
    }
}

pub fn latest() -> HwSample {
    slot()
        .lock()
        .map(|g| g.sample.clone())
        .unwrap_or_default()
}

pub(crate) fn format_dxgi_luid(low: u32, high: i32) -> String {
    format!("0x{:08X}_0x{:08X}", high as u32, low)
}

pub(crate) fn luid_after_token(name: &str) -> Option<&str> {
    let rest = if let Some((_, rest)) = name.split_once("_luid_") {
        rest
    } else {
        name.strip_prefix("luid_")?
    };
    let mut underscores = 0;
    let end = rest
        .char_indices()
        .find(|(_, c)| {
            if *c == '_' {
                underscores += 1;
                underscores == 2
            } else {
                false
            }
        })
        .map(|(i, _)| i)
        .unwrap_or(rest.len());
    if end == 0 { None } else { Some(&rest[..end]) }
}

pub(crate) fn engtype_of(name: &str) -> Option<&str> {
    let rest = name.split_once("engtype_")?.1;
    if rest.is_empty() { None } else { Some(rest) }
}

pub(crate) fn gpu_percent(delta_running: f64, delta_qpc: i64, freq: i64) -> Option<f64> {
    if freq <= 0 || delta_qpc <= 0 || delta_running < 0.0 {
        return None;
    }
    let mut busy_us = delta_running / 10.0;
    let wall_us = delta_qpc as f64 * (1_000_000.0 / freq as f64);
    if wall_us <= 0.0 {
        return None;
    }
    if busy_us > wall_us {
        busy_us = wall_us;
    }
    Some(busy_us / wall_us * 100.0)
}

fn poll_loop(slot: Arc<Mutex<Slot>>) {
    let q = match PdhQuery::open() {
        Some(q) => q,
        None => loop {
            publish_ram_only(&slot);
            thread::sleep(POLL);
        },
    };
    let mut prev_gpu: Option<(HashMap<(String, String), f64>, i64)> = None;
    let mut gpu_ready = false;
    let mut cpu_ready = false;
    loop {
        let ram = read_ram();
        let qpc = qpc_now();
        if q.collect() {
            let gpu_sums = q.read_gpu_engine();
            let cpu = if cpu_ready {
                q.read_cpu()
            } else {
                cpu_ready = true;
                None
            };
            let vram = {
                let want = slot.lock().ok().and_then(|g| g.luid);
                q.read_vram(want)
            };
            let gpu = match (gpu_ready, prev_gpu.take(), qpc) {
                (true, Some((prev, prev_qpc)), Some(now)) => {
                    let freq = qpc_freq().unwrap_or(0);
                    let want = slot.lock().ok().and_then(|g| g.luid);
                    compute_gpu(&gpu_sums, &prev, now - prev_qpc, freq, want)
                }
                _ => {
                    gpu_ready = true;
                    None
                }
            };
            if let Some(now) = qpc {
                prev_gpu = Some((gpu_sums, now));
            }
            if let Ok(mut g) = slot.lock() {
                g.sample = HwSample {
                    gpu_util: gpu,
                    cpu_util: cpu,
                    ram_used_mb: ram.map(|r| r.0),
                    ram_total_mb: ram.map(|r| r.1),
                    vram_dedicated_mb: vram.0,
                    vram_shared_mb: vram.1,
                };
            }
        } else if let Ok(mut g) = slot.lock() {
            g.sample.ram_used_mb = ram.map(|r| r.0);
            g.sample.ram_total_mb = ram.map(|r| r.1);
        }
        thread::sleep(POLL);
    }
}

fn publish_ram_only(slot: &Arc<Mutex<Slot>>) {
    let ram = read_ram();
    if let Ok(mut g) = slot.lock() {
        g.sample.ram_used_mb = ram.map(|r| r.0);
        g.sample.ram_total_mb = ram.map(|r| r.1);
    }
}

fn compute_gpu(
    now: &HashMap<(String, String), f64>,
    prev: &HashMap<(String, String), f64>,
    delta_qpc: i64,
    freq: i64,
    want: Option<(u32, i32)>,
) -> Option<f64> {
    let mut by_luid: HashMap<String, f64> = HashMap::new();
    for (key, running) in now {
        let Some(old) = prev.get(key) else {
            continue;
        };
        let Some(pct) = gpu_percent(running - old, delta_qpc, freq) else {
            continue;
        };
        let e = by_luid.entry(key.0.clone()).or_insert(0.0);
        if pct > *e {
            *e = pct;
        }
    }
    if let Some((low, high)) = want {
        return by_luid.get(&format_dxgi_luid(low, high)).copied();
    }
    by_luid.values().copied().reduce(f64::max)
}

fn read_ram() -> Option<(f64, f64)> {
    unsafe {
        let mut buf = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        GlobalMemoryStatusEx(&mut buf).ok()?;
        if buf.ullTotalPhys == 0 {
            return None;
        }
        let total = buf.ullTotalPhys as f64 / 1_048_576.0;
        let used = buf.ullTotalPhys.saturating_sub(buf.ullAvailPhys) as f64 / 1_048_576.0;
        Some((used, total))
    }
}

fn qpc_now() -> Option<i64> {
    let mut v = 0i64;
    unsafe { QueryPerformanceCounter(&mut v) }.ok()?;
    Some(v)
}

fn qpc_freq() -> Option<i64> {
    let mut v = 0i64;
    unsafe { QueryPerformanceFrequency(&mut v) }.ok()?;
    Some(v)
}

struct PdhQuery {
    query: PDH_HQUERY,
    gpu_engine: Option<PDH_HCOUNTER>,
    cpu_util: Option<PDH_HCOUNTER>,
    vram_ded: Option<PDH_HCOUNTER>,
    vram_shared: Option<PDH_HCOUNTER>,
}

impl PdhQuery {
    fn open() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryA(PCSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let add = |path: PCSTR| {
                let mut c = PDH_HCOUNTER::default();
                if PdhAddEnglishCounterA(query, path, 0, &mut c) == 0 {
                    Some(c)
                } else {
                    None
                }
            };
            let q = Self {
                query,
                gpu_engine: add(s!("\\GPU Engine(*)\\Running Time")),
                // Utility-only until IDA CPU combo (Utility+Performance+Frequency) is confirmed.
                cpu_util: add(s!("\\Processor Information(*)\\% Processor Utility")),
                vram_ded: add(s!("\\GPU Adapter Memory(*)\\Dedicated Usage")),
                vram_shared: add(s!("\\GPU Adapter Memory(*)\\Shared Usage")),
            };
            let _ = PdhCollectQueryData(query);
            Some(q)
        }
    }

    fn collect(&self) -> bool {
        unsafe { PdhCollectQueryData(self.query) == 0 }
    }

    fn read_gpu_engine(&self) -> HashMap<(String, String), f64> {
        let mut out = HashMap::new();
        let Some(c) = self.gpu_engine else {
            return out;
        };
        for (name, val) in formatted_large(c) {
            if name.to_ascii_lowercase().contains("_total") {
                continue;
            }
            let Some(luid) = luid_after_token(&name) else {
                continue;
            };
            let Some(eng) = engtype_of(&name) else {
                continue;
            };
            *out.entry((luid.to_string(), eng.to_string())).or_insert(0.0) += val;
        }
        out
    }

    fn read_cpu(&self) -> Option<f64> {
        let mut sum = 0.0;
        let mut n = 0u32;
        for (name, u) in formatted_double(self.cpu_util?) {
            if name.to_ascii_lowercase().contains("_total") {
                continue;
            }
            if !u.is_finite() || u < 0.0 {
                continue;
            }
            sum += u;
            n += 1;
        }
        if n == 0 {
            None
        } else {
            Some((sum / f64::from(n)).clamp(0.0, 100.0))
        }
    }

    fn read_vram(&self, want: Option<(u32, i32)>) -> (Option<f64>, Option<f64>) {
        let pick = |c: Option<PDH_HCOUNTER>| -> Option<f64> {
            let items = formatted_large(c?);
            let target = want.map(|(lo, hi)| format_dxgi_luid(lo, hi));
            let mut fallback = None;
            for (name, val) in items {
                if name.to_ascii_lowercase().contains("_total") {
                    continue;
                }
                let Some(luid) = luid_after_token(&name) else {
                    continue;
                };
                let mb = val / 1_048_576.0;
                if let Some(t) = &target {
                    if luid == t {
                        return Some(mb);
                    }
                } else {
                    fallback = Some(mb);
                }
            }
            fallback
        };
        (pick(self.vram_ded), pick(self.vram_shared))
    }
}

impl Drop for PdhQuery {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}

fn formatted_large(counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
    formatted(counter, PDH_FMT_LARGE, |item| unsafe {
        item.FmtValue.Anonymous.largeValue as f64
    })
}

fn formatted_double(counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
    formatted(counter, PDH_FMT_DOUBLE, |item| unsafe {
        item.FmtValue.Anonymous.doubleValue
    })
}

fn formatted(
    counter: PDH_HCOUNTER,
    fmt: windows::Win32::System::Performance::PDH_FMT,
    value: fn(&PDH_FMT_COUNTERVALUE_ITEM_A) -> f64,
) -> Vec<(String, f64)> {
    unsafe {
        let mut size = 0u32;
        let mut count = 0u32;
        let st = PdhGetFormattedCounterArrayA(counter, fmt, &mut size, &mut count, None);
        if st != PDH_MORE_DATA || size == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; size as usize];
        let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_A>();
        let st = PdhGetFormattedCounterArrayA(counter, fmt, &mut size, &mut count, Some(items));
        if st != 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for i in 0..count as usize {
            let item = &*items.add(i);
            if item.FmtValue.CStatus != PDH_CSTATUS_VALID_DATA
                && item.FmtValue.CStatus != PDH_CSTATUS_NEW_DATA
            {
                continue;
            }
            let Some(name) = pstr(item.szName) else {
                continue;
            };
            out.push((name, value(item)));
        }
        out
    }
}

fn pstr(p: PSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p.0.cast()) }
        .to_str()
        .ok()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_luid_until_second_underscore() {
        assert_eq!(
            luid_after_token("pid_1_luid_0x00000000_0x00001234_engtype_3D"),
            Some("0x00000000_0x00001234")
        );
        assert_eq!(luid_after_token("engtype_3D"), None);
        assert_eq!(
            luid_after_token("luid_0x00000000_0x00001234"),
            Some("0x00000000_0x00001234")
        );
    }

    #[test]
    fn accepts_every_engtype() {
        assert_eq!(
            engtype_of("pid_1_luid_0x0_0x1_engtype_3D"),
            Some("3D")
        );
        assert_eq!(
            engtype_of("pid_1_luid_0x0_0x1_engtype_Compute"),
            Some("Compute")
        );
        assert_eq!(
            engtype_of("pid_1_luid_0x0_0x1_engtype_Compute_0"),
            Some("Compute_0")
        );
        assert_eq!(
            engtype_of("pid_1_luid_0x0_0x1_engtype_Compute_1"),
            Some("Compute_1")
        );
        assert_eq!(
            engtype_of("pid_1_luid_0x0_0x1_engtype_Copy"),
            Some("Copy")
        );
        assert_eq!(engtype_of("pid_1_luid_0x0_0x1"), None);
        assert_eq!(engtype_of("pid_1_luid_0x0_0x1_engtype_"), None);
    }

    #[test]
    fn dxgi_luid_is_high_then_low() {
        assert_eq!(format_dxgi_luid(0x1234, 0), "0x00000000_0x00001234");
    }

    #[test]
    fn gpu_formula_clamps_and_scales() {
        assert_eq!(gpu_percent(10_000_000.0, 1_000_000, 1_000_000), Some(100.0));
        assert_eq!(gpu_percent(5_000_000.0, 1_000_000, 1_000_000), Some(50.0));
        assert_eq!(gpu_percent(20_000_000.0, 1_000_000, 1_000_000), Some(100.0));
        assert_eq!(gpu_percent(1.0, 0, 1_000_000), None);
    }
}
