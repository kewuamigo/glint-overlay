use core::num::NonZeroU32;
use std::collections::HashMap;

use glint_overlay_common::{
    request::{LayerInputRect, UpdateSharedHandle},
    size::PercentLength,
};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;

use crate::{interop::DxInterop, surface::OverlaySurface};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Renderer {
    Dx12,
    Dx11,
    Dx9,
    Opengl,
    Vulkan,
}

pub struct LayerState {
    pub surface: SurfaceState,
    pub position: (i32, i32),
    /// Explicit percent position from `SetLayerPosition`.
    pub pos_pct: Option<(PercentLength, PercentLength)>,
    /// Optional hit-test rect from `SetLayerInputRect` (draw ignores this).
    pub input_rect: Option<LayerInputRect>,
}

impl LayerState {
    const fn new() -> Self {
        Self {
            surface: SurfaceState::new(),
            position: (0, 0),
            pos_pct: None,
            input_rect: None,
        }
    }
}

pub struct RenderData {
    pub interop: DxInterop,

    /// Layer 0 position (shell); also updated by legacy layout.
    pub position: (i32, i32),
    pub window_size: (u32, u32),
    /// Layer 0 surface (shell); also updated by legacy `update_surface`.
    pub surface: SurfaceState,
    pub renderer: Option<Renderer>,
    /// Layers `>= 1` (layer 0 lives in `surface` / `position`).
    pub layers: HashMap<u32, LayerState>,
    /// Optional percent position for layer 0 from `SetLayerPosition`.
    pub layer0_pos_pct: Option<(PercentLength, PercentLength)>,
    /// Optional hit-test rect for layer 0 from `SetLayerInputRect`.
    pub layer0_input_rect: Option<LayerInputRect>,
}

impl RenderData {
    pub(crate) fn new(interop: DxInterop, window_size: (u32, u32)) -> Self {
        Self {
            interop,
            surface: SurfaceState::new(),
            position: (0, 0),
            window_size,
            renderer: None,
            layers: HashMap::new(),
            layer0_pos_pct: None,
            layer0_input_rect: None,
        }
    }

    pub fn reset(&mut self) {
        self.surface = SurfaceState::new();
        self.position = (0, 0);
        self.layers.clear();
        self.layer0_pos_pct = None;
        self.layer0_input_rect = None;
    }

    pub fn update_surface(&mut self, handle: Option<NonZeroU32>) -> anyhow::Result<()> {
        self.update_layer(0, handle)
    }

    pub fn update_layer(&mut self, layer: u32, handle: Option<NonZeroU32>) -> anyhow::Result<()> {
        if layer == 0 {
            self.surface.update(&self.interop.device, handle)?;
            if handle.is_none() {
                self.layer0_input_rect = None;
            }
        } else {
            let device = self.interop.device.clone();
            let state = self.layers.entry(layer).or_insert_with(LayerState::new);
            state.surface.update(&device, handle)?;
            // Cleared layer must not keep mid-screen position (ghost dock / stale quad).
            if handle.is_none() {
                state.pos_pct = None;
                state.position = (0, 0);
                state.input_rect = None;
            }
        }
        Ok(())
    }

    pub fn set_layer_position_pct(
        &mut self,
        layer: u32,
        x: PercentLength,
        y: PercentLength,
    ) {
        if layer == 0 {
            self.layer0_pos_pct = Some((x, y));
        } else {
            self.layers
                .entry(layer)
                .or_insert_with(LayerState::new)
                .pos_pct = Some((x, y));
        }
    }

    pub fn set_layer_input_rect(&mut self, layer: u32, rect: Option<LayerInputRect>) {
        if layer == 0 {
            self.layer0_input_rect = rect;
        } else {
            self.layers
                .entry(layer)
                .or_insert_with(LayerState::new)
                .input_rect = rect;
        }
    }

    pub fn invalidate_surface(&mut self) {
        self.surface.updated = true;
    }

    /// Layers with a live surface, ascending key order (higher drawn on top).
    ///
    /// `Option<UpdateSharedHandle>` is `Some` only when the shared handle changed
    /// (`take_update`); hooks must not synthesize handles — reopen only on `Some`,
    /// still draw the cached GPU texture when `None`.
    ///
    /// Cleared layers (`UpdateLayerHandle(None)`) still emit one frame with
    /// `handle: None` and zero size so the DXGI texture cache drops the stamp
    /// (otherwise a stale layer-1 quad can look like a second AppShell dock).
    pub fn draw_layers(
        &mut self,
    ) -> Vec<(u32, Option<UpdateSharedHandle>, (i32, i32), (u32, u32))> {
        let mut out = Vec::new();

        {
            let update = self.surface.take_update();
            if let Some(surface) = self.surface.get() {
                out.push((0, update, self.position, surface.size()));
            } else if update.is_some() {
                out.push((0, update, (0, 0), (0, 0)));
            }
        }

        let mut keys: Vec<u32> = self.layers.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            let layer = self.layers.get_mut(&key).unwrap();
            let update = layer.surface.take_update();
            let position = layer.position;
            if let Some(surface) = layer.surface.get() {
                out.push((key, update, position, surface.size()));
            } else if update.is_some() {
                out.push((key, update, (0, 0), (0, 0)));
            }
        }
        out
    }

    /// Snapshot of layer rects for hit-testing: `(layer, x, y, w, h)`, ascending.
    /// Prefers `SetLayerInputRect` when set; otherwise texture size at layer position.
    pub fn layer_rects(&self) -> Vec<(u32, i32, i32, u32, u32)> {
        let mut out = Vec::new();
        if let Some(surface) = self.surface.get() {
            out.push(Self::resolve_hit_rect(
                0,
                self.layer0_input_rect.as_ref(),
                self.position,
                surface.size(),
                self.window_size,
            ));
        }
        let mut keys: Vec<u32> = self
            .layers
            .iter()
            .filter(|(_, l)| l.surface.get().is_some())
            .map(|(k, _)| *k)
            .collect();
        keys.sort_unstable();
        for key in keys {
            let layer = self.layers.get(&key).unwrap();
            let (w, h) = layer.surface.get().unwrap().size();
            out.push(Self::resolve_hit_rect(
                key,
                layer.input_rect.as_ref(),
                layer.position,
                (w, h),
                self.window_size,
            ));
        }
        out
    }

    fn resolve_hit_rect(
        layer: u32,
        input: Option<&LayerInputRect>,
        position: (i32, i32),
        texture_size: (u32, u32),
        window_size: (u32, u32),
    ) -> (u32, i32, i32, u32, u32) {
        if let Some(r) = input {
            (
                layer,
                r.x.resolve(window_size.0 as f32).round() as i32,
                r.y.resolve(window_size.1 as f32).round() as i32,
                r.width,
                r.height,
            )
        } else {
            (layer, position.0, position.1, texture_size.0, texture_size.1)
        }
    }
}

pub struct SurfaceState {
    inner: Option<OverlaySurface>,
    updated: bool,
}

impl SurfaceState {
    const fn new() -> Self {
        Self {
            inner: None,
            updated: true,
        }
    }

    #[inline]
    pub const fn get(&self) -> Option<&OverlaySurface> {
        self.inner.as_ref()
    }

    fn update(&mut self, device: &ID3D11Device, handle: Option<NonZeroU32>) -> anyhow::Result<()> {
        self.updated = true;
        self.inner.take();

        let Some(handle) = handle else {
            return Ok(());
        };

        self.inner = Some(OverlaySurface::open_shared(device, handle.get())?);
        Ok(())
    }

    #[inline]
    pub fn take_update(&mut self) -> Option<UpdateSharedHandle> {
        if self.updated {
            self.updated = false;
            Some(UpdateSharedHandle {
                handle: self.get().map(|surface| surface.shared_handle()),
            })
        } else {
            None
        }
    }

    #[inline]
    pub fn invalidate_update(&mut self) -> bool {
        if self.updated {
            self.updated = false;
            true
        } else {
            false
        }
    }
}
