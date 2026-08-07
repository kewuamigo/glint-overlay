use std::thread;
use std::time::Duration;

use glint_metrics_common::read_metrics_for_pid;
use serde::Serialize;

#[derive(Serialize)]
struct NativeSnapshot {
    native_fps: f32,
    native_frame_count: u64,
    pid: u32,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let pid: u32 = args
        .next()
        .and_then(|s| s.parse().ok())
        .expect("usage: glint-metrics-reader <pid> [--interval-ms N]");
    let interval_ms: u64 = args
        .find(|a| a == "--interval-ms")
        .and_then(|_| args.next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);

    loop {
        if let Some(block) = read_metrics_for_pid(pid) {
            let snap = NativeSnapshot {
                native_fps: block.native_fps,
                native_frame_count: block.native_frame_count,
                pid: block.pid,
            };
            if let Ok(json) = serde_json::to_string(&snap) {
                println!("{json}");
            }
        }
        thread::sleep(Duration::from_millis(interval_ms));
    }
}
