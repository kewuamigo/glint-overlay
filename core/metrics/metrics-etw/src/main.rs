use std::io::{self, Write};
use std::process;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use glint_metrics_etw::{EtwMetricsConsumer, HookedNativeSample};
use glint_metrics_common::{now_ms, read_metrics_for_pid};
use tracing::info;

#[cfg(windows)]
fn ensure_admin() -> Result<()> {
    if is_elevated() {
        return Ok(());
    }

    anyhow::bail!(
        "Administrator privileges required for ETW FPS capture. \
         Start the launcher with UAC elevation."
    );
}

#[cfg(windows)]
fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut return_length = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut return_length,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

fn read_hooked_native(target_pid: u32) -> Option<HookedNativeSample> {
    let block = read_metrics_for_pid(target_pid)?;
    let now = now_ms();

    // Module-scan freshness is tracked separately from the Present hook so the
    // DLSS/FSR label survives even when the hook is not counting frames.
    let fg_kind_fresh = block.fg_updated_at_ms > 0
        && now.saturating_sub(block.fg_updated_at_ms) <= 2_500;

    let hook_fresh = now.saturating_sub(block.updated_at_ms) <= 2_500;
    let fps = f64::from(block.native_fps);
    let hook_valid = hook_fresh && fps > 0.0 && fps.is_finite();

    let game_fps = f64::from(block.game_frame_fps);
    let game_valid = block.game_frame_updated_at_ms > 0
        && now.saturating_sub(block.game_frame_updated_at_ms) <= 2_500
        && game_fps > 0.0
        && game_fps.is_finite();

    if !hook_valid && !fg_kind_fresh && !game_valid {
        return None;
    }

    let frame_time_ms = f64::from(block.native_frame_time_ms);
    Some(HookedNativeSample {
        fps: if hook_valid { fps } else { 0.0 },
        frame_time_ms: if hook_valid && frame_time_ms > 0.0 {
            frame_time_ms
        } else if hook_valid {
            1000.0 / fps
        } else {
            0.0
        },
        fg_kind: block.fg_kind,
        fg_kind_fresh,
        game_fps: if game_valid { game_fps } else { 0.0 },
    })
}

#[derive(Parser)]
#[command(name = "glint-metrics-etw")]
struct Cli {
    /// Target game process ID
    pid: u32,

    /// Poll interval in milliseconds
    #[arg(long, default_value = "500")]
    interval_ms: u64,

    /// Unique suffix for ETW trace session name (avoids AlreadyExist collisions)
    #[arg(long)]
    trace_suffix: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    #[cfg(windows)]
    if let Err(err) = ensure_admin() {
        eprintln!("{err:#}");
        process::exit(1);
    }

    let consumer = std::sync::Arc::new(EtwMetricsConsumer::new(cli.pid));
    let _trace = consumer
        .clone()
        .start(cli.trace_suffix.as_deref())
        .map_err(|err| {
            eprintln!("ETW trace start failed: {err:#}");
            err
        })?;

    info!(
        pid = cli.pid,
        "streaming FPS (Present hook native + ETW flip display) on stdout"
    );

    let write_snapshot = |consumer: &EtwMetricsConsumer| -> Result<()> {
        let hooked = read_hooked_native(cli.pid);
        let snap = consumer.tracker().lock().snapshot(hooked);
        println!("{}", serde_json::to_string(&snap)?);
        io::stdout().flush()?;
        Ok(())
    };

    write_snapshot(&consumer)?;

    let mut interval = tokio::time::interval(Duration::from_millis(cli.interval_ms));
    loop {
        interval.tick().await;
        write_snapshot(&consumer)?;
    }
}
