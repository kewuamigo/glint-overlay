use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::Result;
use ferrisetw::parser::Parser;
use ferrisetw::provider::Provider;
use ferrisetw::schema_locator::SchemaLocator;
use ferrisetw::trace::{stop_trace_by_name, UserTrace};
use ferrisetw::EventRecord;
use parking_lot::Mutex;
use tracing::info;
use windows::Win32::System::Diagnostics::Etw::EVENT_RECORD;

use crate::cleanup::stop_stale_traces;
use crate::fps::{EventCounters, HardwareFpsTracker};

/// Microsoft-Windows-DXGI — Present::Start (42) for DX10/11/12 native FPS
const DXGI_GUID: &str = "CA11C036-0102-4A2D-A6AD-F03CFED5D3C9";
const DXGI_PRESENT_START: u16 = 42;

/// Microsoft-Windows-DxgKrnl — display flips / kernel presents (PresentMon-style)
const DXGKRNL_GUID: &str = "802EC45A-1E99-4B83-9920-87C98277BA9D";
const DXGKRNL_KEYWORD_PRESENT: u64 = 0x8000000;
const DXGKRNL_KEYWORD_BASE: u64 = 0x1;

/// MMIOFlip (116), Flip (168), Present (184), Blit (166)
const FLIP_EVENT_IDS: &[u16] = &[116, 166, 168, 184];
/// PresentHistoryStart (171), PresentHistory (172)
const PRESENT_EVENT_IDS: &[u16] = &[171, 172];

/// Intel-PresentMon — per-frame FrameType tags emitted by drivers/SDKs
/// (Intel XeFG and AMD AFMF instrument this provider; see PresentMon
/// `--track_frame_type`). Keyword 0x1 = FrameTypes.
const INTEL_PRESENTMON_GUID: &str = "ECAA4712-4644-442F-B94C-A32F6CF8A499";
const INTEL_PM_KEYWORD_FRAME_TYPES: u64 = 0x1;
/// PresentFrameType (per present, emitted on the game's present thread)
const PM_PRESENT_FRAME_TYPE: u16 = 1;
/// FlipFrameType (per displayed flip, emitted by the driver)
const PM_FLIP_FRAME_TYPE: u16 = 2;

/// `PresentFrameType_Info_Props`: FrameId(u32) + FrameType(u8) [+ AppFrameId(u32) in v1]
const PM_PRESENT_FRAME_TYPE_TAG_OFFSET: usize = 4;
/// `FlipFrameType_Info_Props`: VidPnSourceId(u32) + LayerIndex(u32) + PresentId(u64) + FrameType(u8)
const PM_FLIP_FRAME_TYPE_TAG_OFFSET: usize = 16;

pub struct EtwMetricsConsumer {
    target_pid: u32,
    tracker: Arc<Mutex<HardwareFpsTracker>>,
    counters: Arc<EventCounters>,
}

impl EtwMetricsConsumer {
    pub fn new(target_pid: u32) -> Self {
        let counters = Arc::new(EventCounters::default());
        Self {
            target_pid,
            tracker: Arc::new(Mutex::new(HardwareFpsTracker::new(counters.clone()))),
            counters,
        }
    }

    pub fn tracker(&self) -> Arc<Mutex<HardwareFpsTracker>> {
        self.tracker.clone()
    }

    pub fn start(self: Arc<Self>, trace_suffix: Option<&str>) -> Result<UserTrace> {
        let target_pid = self.target_pid;

        stop_stale_traces();

        let trace_name = match trace_suffix {
            Some(suffix) => format!("GlintMetrics-{target_pid}-{suffix}"),
            None => format!("GlintMetrics-{target_pid}-{}", std::process::id()),
        };

        let _ = stop_trace_by_name(&trace_name);

        let native_tracker = self.tracker.clone();
        let dxgi_counter = self.counters.clone();
        let dxgi_provider = Provider::by_guid(DXGI_GUID)
            .add_callback(move |record: &EventRecord, _schema_locator: &SchemaLocator| {
                if record.event_id() != DXGI_PRESENT_START {
                    return;
                }
                if record.process_id() != target_pid {
                    return;
                }
                dxgi_counter.dxgi.fetch_add(1, Ordering::Relaxed);
                native_tracker.lock().on_dxgi_present();
            })
            .build();

        let flip_tracker = self.tracker.clone();
        let dxgk_flip_counter = self.counters.clone();
        let present_hist_tracker = self.tracker.clone();
        let dxgk_present_counter = self.counters.clone();
        let dxgk_provider = Provider::by_guid(DXGKRNL_GUID)
            .any(DXGKRNL_KEYWORD_PRESENT | DXGKRNL_KEYWORD_BASE)
            .add_callback(move |record: &EventRecord, schema_locator: &SchemaLocator| {
                let event_id = record.event_id();
                let is_flip = FLIP_EVENT_IDS.contains(&event_id);
                let is_present = PRESENT_EVENT_IDS.contains(&event_id);
                if !is_flip && !is_present {
                    return;
                }

                if !event_matches_target(record, schema_locator, target_pid) {
                    return;
                }

                if is_flip {
                    dxgk_flip_counter.dxgk_flip.fetch_add(1, Ordering::Relaxed);
                    flip_tracker.lock().on_flip();
                } else {
                    dxgk_present_counter.dxgk_present.fetch_add(1, Ordering::Relaxed);
                    present_hist_tracker.lock().on_present_history();
                }
            })
            .build();

        let tag_tracker = self.tracker.clone();
        let intel_pm_provider = Provider::by_guid(INTEL_PRESENTMON_GUID)
            .any(INTEL_PM_KEYWORD_FRAME_TYPES)
            .add_callback(move |record: &EventRecord, _schema_locator: &SchemaLocator| {
                match record.event_id() {
                    PM_PRESENT_FRAME_TYPE => {
                        // Emitted in-process by the FG SDK on the present thread,
                        // so the record PID identifies the game directly.
                        if record.process_id() != target_pid {
                            return;
                        }
                        if let Some(tag) =
                            read_payload_u8(record, PM_PRESENT_FRAME_TYPE_TAG_OFFSET)
                        {
                            tag_tracker.lock().on_tagged_present(tag);
                        }
                    }
                    PM_FLIP_FRAME_TYPE => {
                        // Driver-emitted (e.g. AMD AFMF) with no game PID attached.
                        // Full attribution needs the PresentMon MPO3/PresentId state
                        // machine; we accept all tags since only the FG-active
                        // fullscreen game produces them in practice.
                        if let Some(tag) = read_payload_u8(record, PM_FLIP_FRAME_TYPE_TAG_OFFSET) {
                            tag_tracker.lock().on_tagged_flip(tag);
                        }
                    }
                    _ => {}
                }
            })
            .build();

        let trace = UserTrace::new()
            .named(trace_name.clone())
            .enable(dxgi_provider)
            .enable(dxgk_provider)
            .enable(intel_pm_provider)
            .start_and_process()
            .map_err(|e| anyhow::anyhow!("ETW trace start failed ({trace_name}): {e:?}"))?;

        info!(
            target_pid,
            trace = %trace_name,
            "ETW DXGI+DxgKrnl+Intel-PresentMon consumer started"
        );
        Ok(trace)
    }
}

fn event_matches_target(
    record: &EventRecord,
    schema_locator: &SchemaLocator,
    target_pid: u32,
) -> bool {
    if record.process_id() == target_pid {
        return true;
    }

    if let Ok(schema) = schema_locator.event_schema(record) {
        let parser = Parser::create(record, &schema);
        for field in ["ProcessId", "hProcessId"] {
            if let Ok(pid) = parser.try_parse::<u32>(field) {
                return pid == target_pid;
            }
        }
    }

    false
}

/// Read one byte from the raw event payload at `offset`.
///
/// The Intel-PresentMon provider has no installed manifest, so TDH schema
/// lookups fail; PresentMon itself casts the raw `UserData` structs and we do
/// the same. `EventRecord` is `#[repr(transparent)]` over `EVENT_RECORD`.
fn read_payload_u8(record: &EventRecord, offset: usize) -> Option<u8> {
    let raw = unsafe { &*(record as *const EventRecord).cast::<EVENT_RECORD>() };
    if raw.UserData.is_null() {
        return None;
    }
    let len = raw.UserDataLength as usize;
    if offset >= len {
        return None;
    }
    let data = unsafe { std::slice::from_raw_parts(raw.UserData.cast::<u8>(), len) };
    Some(data[offset])
}
