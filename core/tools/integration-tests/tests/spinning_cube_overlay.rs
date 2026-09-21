//! Inject overlay into SpinningCube.exe and verify IPC + window events.

use std::time::Duration;

use glint_integration_tests::{
    attach_spinning_cube_overlay, integration_enabled, resize_game_window, skip_unless_integration,
    wait_for_window_added, wait_for_window_resized,
};
use glint_overlay_common::request::{
    ListenInput, SetAnchor, SetMargin, SetPosition, UpdateSharedHandle,
};
use glint_overlay_common::size::PercentLength;

#[tokio::test]
async fn spinning_cube_window_resize_event() -> anyhow::Result<()> {
    if skip_unless_integration() {
        return Ok(());
    }

    let (_proc, _pid, _conn, mut events) = attach_spinning_cube_overlay().await?;
    let (hwnd, width, height, _) =
        wait_for_window_added(&mut events, Duration::from_secs(15)).await?;

    let (mut target_w, mut target_h) = (width.saturating_sub(64), height.saturating_sub(48));
    if target_w == width || target_h == height {
        target_w = width.saturating_add(64).max(320);
        target_h = height.saturating_add(48).max(240);
    }
    resize_game_window(hwnd, target_w as i32, target_h as i32)?;

    let (rw, rh) = wait_for_window_resized(
        &mut events,
        hwnd,
        Some((target_w, target_h)),
        Duration::from_secs(10),
    )
    .await?;

    assert_eq!((rw, rh), (target_w, target_h));
    Ok(())
}

#[tokio::test]
async fn spinning_cube_layout_ipc_stack() -> anyhow::Result<()> {
    if skip_unless_integration() {
        return Ok(());
    }

    let (_proc, _pid, mut conn, mut events) = attach_spinning_cube_overlay().await?;
    let (hwnd, _, _, _) = wait_for_window_added(&mut events, Duration::from_secs(15)).await?;

    assert!(
        conn.window(hwnd)
            .request(SetAnchor {
                x: PercentLength::Percent(0.5),
                y: PercentLength::Percent(0.5),
            })
            .await?
    );
    assert!(
        conn.window(hwnd)
            .request(SetMargin {
                top: PercentLength::Length(0.0),
                right: PercentLength::Length(0.0),
                bottom: PercentLength::Length(0.0),
                left: PercentLength::Length(0.0),
            })
            .await?
    );
    assert!(
        conn.window(hwnd)
            .request(SetPosition {
                x: PercentLength::Percent(0.5),
                y: PercentLength::Percent(0.5),
            })
            .await?
    );
    // Clear host texture binding (paint path noop without a real shared handle).
    assert!(
        conn.window(hwnd)
            .request(UpdateSharedHandle { handle: None })
            .await?
    );

    Ok(())
}

#[tokio::test]
async fn spinning_cube_inject_ipc_roundtrip() -> anyhow::Result<()> {
    if skip_unless_integration() {
        return Ok(());
    }
    assert!(integration_enabled());

    let (_proc, _pid, mut conn, mut events) = attach_spinning_cube_overlay().await?;

    let (hwnd, _width, _height, gpu_id) =
        wait_for_window_added(&mut events, Duration::from_secs(15)).await?;
    assert!(
        gpu_id.low != 0 || gpu_id.high != 0,
        "Added event should report adapter LUID for D3D11 swapchain"
    );

    let ok = conn
        .window(hwnd)
        .request(SetPosition {
            x: PercentLength::Percent(0.0),
            y: PercentLength::Percent(0.0),
        })
        .await?;
    assert!(ok, "SetPosition should return true");

    let listen_ok = conn
        .window(hwnd)
        .request(ListenInput {
            cursor: true,
            keyboard: false,
        })
        .await?;
    assert!(listen_ok, "ListenInput should return true");

    Ok(())
}

#[tokio::test]
async fn spinning_cube_process_alive_during_ipc() -> anyhow::Result<()> {
    if skip_unless_integration() {
        return Ok(());
    }

    let (_proc, pid, _conn, mut events) = attach_spinning_cube_overlay().await?;
    let (_hwnd, width, height, _) =
        wait_for_window_added(&mut events, Duration::from_secs(15)).await?;

    assert!(glint_injector::is_process_alive(pid));
    assert!(width > 0 && height > 0);

    Ok(())
}
