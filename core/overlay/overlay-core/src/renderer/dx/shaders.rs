// Compiled with `fxc /T vs_5_0 /O3 /Fo texture_vs.o texture.hlsl /E vs_main`
pub const VERTEX_SHADER: &[u8] = include_bytes!("shaders/texture_vs.o");

// Compiled with `fxc /T ps_5_0 /O3 /Fo texture_ps.o texture.hlsl /E ps_main`
pub const PIXEL_SHADER: &[u8] = include_bytes!("shaders/texture_ps.o");

// Compiled with `fxc /T ps_5_0 /O3 /Fo texture_ps_scrgb.o texture.hlsl /E ps_scrgb`
pub const PIXEL_SHADER_SCRGB: &[u8] = include_bytes!("shaders/texture_ps_scrgb.o");

// Compiled with `fxc /T ps_5_0 /O3 /Fo texture_ps_pq.o texture.hlsl /E ps_pq`
pub const PIXEL_SHADER_PQ: &[u8] = include_bytes!("shaders/texture_ps_pq.o");

pub const VERTEX_SHADER_4: &[u8] = include_bytes!("shaders/texture_vs_4.o");
pub const PIXEL_SHADER_4: &[u8] = include_bytes!("shaders/texture_ps_4.o");
pub const PIXEL_SHADER_SCRGB_4: &[u8] = include_bytes!("shaders/texture_ps_scrgb_4.o");
pub const PIXEL_SHADER_PQ_4: &[u8] = include_bytes!("shaders/texture_ps_pq_4.o");
