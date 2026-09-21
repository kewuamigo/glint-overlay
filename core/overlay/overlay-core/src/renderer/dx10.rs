use core::{mem, num::NonZeroU32, ptr};
use std::collections::HashMap;

use anyhow::Context;
use glint_overlay_common::request::UpdateSharedHandle;
use windows::{
    Win32::{
        Foundation::HANDLE,
        Graphics::{
            Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP, D3D_SRV_DIMENSION_TEXTURE2D},
            Direct3D10::*,
            Dxgi::{Common::DXGI_FORMAT, IDXGIKeyedMutex},
        },
    },
    core::{BOOL, Interface},
};

use crate::{
    renderer::{dx::shaders, hdr::OverlayBlitColorSpace},
    texture::OverlayTextureState,
    util::{MailboxSample, mailbox_sample, with_keyed_mutex_sampled},
};

const SAMPLER_DESC: D3D10_SAMPLER_DESC = D3D10_SAMPLER_DESC {
    Filter: D3D10_FILTER_MIN_MAG_MIP_POINT,
    AddressU: D3D10_TEXTURE_ADDRESS_CLAMP,
    AddressV: D3D10_TEXTURE_ADDRESS_CLAMP,
    AddressW: D3D10_TEXTURE_ADDRESS_CLAMP,
    MipLODBias: 0.0,
    MaxAnisotropy: 0,
    ComparisonFunc: D3D10_COMPARISON_NEVER,
    BorderColor: [0.0; 4],
    MinLOD: 0.0,
    MaxLOD: D3D10_FLOAT32_MAX,
};

struct Dx10Tex {
    texture: ID3D10Texture2D,
    mutex: Option<IDXGIKeyedMutex>,
    view: ID3D10ShaderResourceView,
}

struct LastGood {
    tex: ID3D10Texture2D,
    view: ID3D10ShaderResourceView,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
}

pub struct Dx10Renderer {
    constant_buffer: ID3D10Buffer,
    textures: HashMap<u32, OverlayTextureState<Dx10Tex>>,
    last_good: HashMap<u32, LastGood>,
    vertex_shader: ID3D10VertexShader,
    pixel_shader: ID3D10PixelShader,
    pixel_shader_scrgb: ID3D10PixelShader,
    pixel_shader_pq: ID3D10PixelShader,
    blend_alpha: ID3D10BlendState,
    sampler_state: ID3D10SamplerState,
}

impl Dx10Renderer {
    #[tracing::instrument]
    pub fn new(device: &ID3D10Device) -> anyhow::Result<Self> {
        unsafe {
            let mut vertex_shader = None;
            device
                .CreateVertexShader(shaders::VERTEX_SHADER_4, Some(&mut vertex_shader))
                .context("vertex shader failed to link")?;
            let vertex_shader = vertex_shader.unwrap();

            let mut pixel_shader = None;
            device
                .CreatePixelShader(shaders::PIXEL_SHADER_4, Some(&mut pixel_shader))
                .context("pixel shader failed to link")?;
            let pixel_shader = pixel_shader.unwrap();

            let mut pixel_shader_scrgb = None;
            device
                .CreatePixelShader(shaders::PIXEL_SHADER_SCRGB_4, Some(&mut pixel_shader_scrgb))
                .context("scrgb pixel shader failed to link")?;
            let pixel_shader_scrgb = pixel_shader_scrgb.unwrap();

            let mut pixel_shader_pq = None;
            device
                .CreatePixelShader(shaders::PIXEL_SHADER_PQ_4, Some(&mut pixel_shader_pq))
                .context("pq pixel shader failed to link")?;
            let pixel_shader_pq = pixel_shader_pq.unwrap();

            let mut constant_buffer = None;
            device
                .CreateBuffer(
                    &D3D10_BUFFER_DESC {
                        ByteWidth: mem::size_of::<[f32; 4]>() as _,
                        Usage: D3D10_USAGE_DYNAMIC,
                        BindFlags: D3D10_BIND_CONSTANT_BUFFER.0 as _,
                        CPUAccessFlags: D3D10_CPU_ACCESS_WRITE.0 as _,
                        MiscFlags: 0,
                    },
                    None,
                    Some(&mut constant_buffer),
                )
                .context("cannot create constant buffer")?;
            let constant_buffer = constant_buffer.unwrap();

            let mut blend_alpha = None;
            device
                .CreateBlendState(
                    &D3D10_BLEND_DESC {
                        AlphaToCoverageEnable: BOOL(0),
                        BlendEnable: [BOOL(1); 8],
                        SrcBlend: D3D10_BLEND_SRC_ALPHA,
                        DestBlend: D3D10_BLEND_INV_SRC_ALPHA,
                        BlendOp: D3D10_BLEND_OP_ADD,
                        SrcBlendAlpha: D3D10_BLEND_ONE,
                        DestBlendAlpha: D3D10_BLEND_INV_SRC_ALPHA,
                        BlendOpAlpha: D3D10_BLEND_OP_ADD,
                        RenderTargetWriteMask: [D3D10_COLOR_WRITE_ENABLE_ALL.0 as u8; 8],
                    },
                    Some(&mut blend_alpha),
                )
                .context("cannot create alpha blend state")?;
            let blend_alpha = blend_alpha.unwrap();

            let mut sampler_state = None;
            device
                .CreateSamplerState(&SAMPLER_DESC, Some(&mut sampler_state))
                .context("cannot create sampler state")?;
            let sampler_state = sampler_state.unwrap();

            Ok(Self {
                constant_buffer,
                textures: HashMap::new(),
                last_good: HashMap::new(),
                vertex_shader,
                pixel_shader,
                pixel_shader_scrgb,
                pixel_shader_pq,
                blend_alpha,
                sampler_state,
            })
        }
    }

    pub fn update_texture(&mut self, layer: u32, shared: UpdateSharedHandle) {
        if shared.handle.is_none() {
            self.textures.remove(&layer);
            self.last_good.remove(&layer);
            return;
        }
        self.textures.entry(layer).or_default().update(shared);
    }

    #[tracing::instrument(skip(self))]
    pub fn draw(
        &mut self,
        device: &ID3D10Device,
        layer: u32,
        position: (i32, i32),
        size: (u32, u32),
        screen: (u32, u32),
        color_space: OverlayBlitColorSpace,
    ) -> anyhow::Result<()> {
        if screen.0 == 0 || screen.1 == 0 {
            return Ok(());
        }

        let (live_tex, live_view, mutex) = {
            let Some(tex) = self
                .textures
                .entry(layer)
                .or_default()
                .get_or_create(|handle| open_shared_texture(device, handle))?
            else {
                return Ok(());
            };
            (tex.texture.clone(), tex.view.clone(), tex.mutex.clone())
        };

        let has_cache = self.last_good.contains_key(&layer);
        let rect = [
            (position.0 as f32 / screen.0 as f32) * 2.0 - 1.0,
            -(position.1 as f32 / screen.1 as f32) * 2.0 + 1.0,
            (size.0 as f32 / screen.0 as f32) * 2.0,
            -(size.1 as f32 / screen.1 as f32) * 2.0,
        ];

        unsafe {
            let mut mapped = ptr::null_mut();
            self.constant_buffer
                .Map(D3D10_MAP_WRITE_DISCARD, 0, &mut mapped)?;
            mapped.cast::<[f32; 4]>().write(rect);
            self.constant_buffer.Unmap();

            device.OMSetBlendState(&self.blend_alpha, &[0.0; 4], 0x00ffffff);
            device.RSSetViewports(Some(&[D3D10_VIEWPORT {
                TopLeftX: 0,
                TopLeftY: 0,
                Width: screen.0,
                Height: screen.1,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));

            device.VSSetShader(&self.vertex_shader);
            device.PSSetShader(self.pixel_shader(color_space));
            device.PSSetSamplers(0, Some(&[Some(self.sampler_state.clone())]));
            device.VSSetConstantBuffers(0, Some(&[Some(self.constant_buffer.clone())]));
            device.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);

            let cache_view = self.last_good.get(&layer).map(|g| g.view.clone());
            with_keyed_mutex_sampled(mutex.as_ref(), |lock_held| {
                let sample = mailbox_sample(lock_held, has_cache);
                match sample {
                    MailboxSample::Skip => {}
                    MailboxSample::Cache => {
                        let srv = cache_view.clone().unwrap_or_else(|| live_view.clone());
                        device.PSSetShaderResources(0, Some(&[Some(srv)]));
                        device.Draw(4, 0);
                    }
                    MailboxSample::Live => {
                        device.PSSetShaderResources(0, Some(&[Some(live_view.clone())]));
                        device.Draw(4, 0);
                        if lock_held {
                            snapshot_last_good(device, &mut self.last_good, layer, &live_tex);
                        }
                    }
                }
            })?;
        }

        Ok(())
    }

    fn pixel_shader(&self, color_space: OverlayBlitColorSpace) -> &ID3D10PixelShader {
        match color_space {
            OverlayBlitColorSpace::Sdr => &self.pixel_shader,
            OverlayBlitColorSpace::Scrgb => &self.pixel_shader_scrgb,
            OverlayBlitColorSpace::Pq => &self.pixel_shader_pq,
        }
    }
}

fn open_shared_texture(
    device: &ID3D10Device,
    handle: NonZeroU32,
) -> anyhow::Result<Option<Dx10Tex>> {
    let nt = HANDLE(ptr::with_exposed_provenance_mut(handle.get() as usize));
    let mut raw: *mut core::ffi::c_void = ptr::null_mut();
    if unsafe { device.OpenSharedResource(nt, &ID3D10Texture2D::IID, Some(&mut raw)) }.is_err()
        || raw.is_null()
    {
        return Ok(None);
    }
    let texture = unsafe { ID3D10Texture2D::from_raw(raw) };

    let mut desc = D3D10_TEXTURE2D_DESC::default();
    unsafe {
        texture.GetDesc(&mut desc);
    }
    if desc.Width == 0 || desc.Height == 0 {
        return Ok(None);
    }

    let mutex = texture.cast::<IDXGIKeyedMutex>().ok();
    let mut view = None;
    unsafe {
        device.CreateShaderResourceView(
            &texture,
            Some(&D3D10_SHADER_RESOURCE_VIEW_DESC {
                Format: desc.Format,
                ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
                Anonymous: D3D10_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D10_TEX2D_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                    },
                },
            }),
            Some(&mut view),
        )?;
    }
    let view = view.context("cannot create texture view")?;

    Ok(Some(Dx10Tex {
        texture,
        mutex,
        view,
    }))
}

fn snapshot_last_good(
    device: &ID3D10Device,
    last_good: &mut HashMap<u32, LastGood>,
    layer: u32,
    src: &ID3D10Texture2D,
) {
    let mut desc = D3D10_TEXTURE2D_DESC::default();
    unsafe {
        src.GetDesc(&mut desc);
    }
    if desc.Width == 0 || desc.Height == 0 {
        return;
    }

    let reuse = last_good.get(&layer).is_some_and(|g| {
        g.width == desc.Width && g.height == desc.Height && g.format == desc.Format
    });
    if !reuse {
        desc.MiscFlags = 0;
        desc.BindFlags = D3D10_BIND_SHADER_RESOURCE.0 as u32;
        desc.CPUAccessFlags = 0;
        desc.Usage = D3D10_USAGE_DEFAULT;
        let Ok(tex) = (unsafe { device.CreateTexture2D(&desc, None) }) else {
            return;
        };
        let mut view = None;
        if unsafe {
            device.CreateShaderResourceView(
                &tex,
                Some(&D3D10_SHADER_RESOURCE_VIEW_DESC {
                    Format: desc.Format,
                    ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D10_SHADER_RESOURCE_VIEW_DESC_0 {
                        Texture2D: D3D10_TEX2D_SRV {
                            MostDetailedMip: 0,
                            MipLevels: 1,
                        },
                    },
                }),
                Some(&mut view),
            )
        }
        .is_err()
        {
            return;
        }
        let Some(view) = view else {
            return;
        };
        last_good.insert(
            layer,
            LastGood {
                tex,
                view,
                width: desc.Width,
                height: desc.Height,
                format: desc.Format,
            },
        );
    }
    if let Some(good) = last_good.get(&layer) {
        unsafe {
            device.CopyResource(&good.tex, src);
        }
    }
}
