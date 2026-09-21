//! Inject metrics-native DLL and read shared-memory FPS block.

use std::time::Duration;

use glint_injector::{default_metrics_dll, inject_metrics_dll};
use glint_integration_tests::{
    attach_spinning_cube_overlay, skip_unless_integration, wait_for_window_added,
};
use glint_metrics_common::read_metrics_for_pid;

#[tokio::test]
async fn spinning_cube_metrics_shm_after_present() -> anyhow::Result<()> {
    if skip_unless_integration() {
        return Ok(());
    }

    let metrics = default_metrics_dll().ok_or_else(|| {
        anyhow::anyhow!("metrics DLL not built — cargo build -p glint-metrics-native")
    })?;

    let (_proc, pid, _conn, mut events) = attach_spinning_cube_overlay().await?;
    wait_for_window_added(&mut events, Duration::from_secs(15)).await?;

    inject_metrics_dll(pid, &metrics)?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_metrics = false;
    while tokio::time::Instant::now() < deadline {
        if let Some(block) = read_metrics_for_pid(pid) {
            if block.version > 0 {
                saw_metrics = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    assert!(
        saw_metrics,
        "expected MetricsBlock in SHM after metrics DLL inject (game must be presenting frames)"
    );

    Ok(())
}
