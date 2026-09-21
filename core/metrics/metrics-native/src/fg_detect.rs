//! In-process frame-generation SDK detection via loaded-module scan.
//!
//! With NVIDIA DLSS-FG the Streamline interposer gives the game a proxy
//! swapchain: the game's `Present` calls (which we hook) run at native rate
//! while the real swapchain presents at the generated rate. The loaded FG
//! module tells us which SDK is responsible, Steam-overlay style.

use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::core::PCWSTR;

pub const FG_NONE: u32 = 0;
pub const FG_DLSS: u32 = 1;
pub const FG_FSR: u32 = 2;
pub const FG_XEFG: u32 = 3;

/// (module name, kind) — first match wins.
const FG_MODULES: &[(&str, u32)] = &[
    // NVIDIA DLSS Frame Generation (Streamline plugin + NGX runtime)
    ("sl.dlss_g.dll", FG_DLSS),
    ("nvngx_dlssg.dll", FG_DLSS),
    // AMD FSR 3 / FidelityFX frame interpolation
    ("ffx_frameinterpolation_x64.dll", FG_FSR),
    ("ffx_fsr3_x64.dll", FG_FSR),
    ("amd_fidelityfx_dx12.dll", FG_FSR),
    ("amd_fidelityfx_vk.dll", FG_FSR),
    // Intel XeSS Frame Generation
    ("libxess_fg.dll", FG_XEFG),
];

/// Scan for known FG SDK modules in the current process.
///
/// `GetModuleHandleW` does not load anything and only touches the loader
/// tables, so this is cheap enough to call periodically from the present hook.
pub fn detect_fg_kind() -> u32 {
    for (name, kind) in FG_MODULES {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        if unsafe { GetModuleHandleW(PCWSTR(wide.as_ptr())) }.is_ok() {
            return *kind;
        }
    }
    FG_NONE
}
