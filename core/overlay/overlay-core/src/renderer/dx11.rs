use core::{mem, num::NonZeroU32};
use std::collections::HashMap;

use anyhow::Context;
use glint_overlay_common::request::UpdateSharedHandle;
use windows::{
    Win32::{
        Graphics::{
            Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP, D3D_SRV_DIMENSION_TEXTURE2D},
            Direct3D11::*,
            Dxgi::IDXGIKeyedMutex,
        },
    },
    core::{BOOL, Interface},
};

use crate::{renderer::dx::shaders, texture::OverlayTextureState, util::with_keyed_mutex};

const SAMPLER_DESC: D3D11_SAMPLER_DESC = D3D11_SAMPLER_DESC {
    Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
    MipLODBias: 0.0,
    MaxAnisotropy: 0,
    ComparisonFunc: D3D11_COMPARISON_NEVER,
    BorderColor: [0.0; 4],
    MinLOD: 0.0,
    MaxLOD: D3D11_FLOAT32_MAX,
};

struct Dx11Tex {
    mutex: Option<IDXGIKeyedMutex>,
    view: ID3D11ShaderResourceView,
}

pub struct Dx11Renderer {
    constant_buffer: ID3D11Buffer,
    /// Per-layer shared-texture cache (update only when handle changes).
    textures: HashMap<u32, OverlayTextureState<Dx11Tex>>,

    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    /// All layers alpha-blend (L1 CEF is fullscreen with transparent outside the inner window).
    blend_alpha: ID3D11BlendState,
    sampler_state: ID3D11SamplerState,
}

impl Dx11Renderer {
    #[tracing::instrument]
    pub fn new(device: &ID3D11Device) -> anyhow::Result<Self> {
        unsafe {
            let mut vertex_shader = None;
            device
                .CreateVertexShader(shaders::VERTEX_SHADER, None, Some(&mut vertex_shader))
                .context("vertex shader failed to link")?;
            let vertex_shader = vertex_shader.unwrap();

            let mut pixel_shader = None;
            device
                .CreatePixelShader(shaders::PIXEL_SHADER, None, Some(&mut pixel_shader))
                .context("pixel shader failed to link")?;
            let pixel_shader = pixel_shader.unwrap();

            let mut constant_buffer = None;
            device
                .CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        ByteWidth: mem::size_of::<[f32; 4]>() as _,
                        Usage: D3D11_USAGE_DYNAMIC,
                        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as _,
                        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as _,
                        MiscFlags: 0,
                        StructureByteStride: 0,
                    },
                    None,
                    Some(&mut constant_buffer),
                )
                .context("cannot create constant buffer")?;
            let constant_buffer = constant_buffer.unwrap();

            let mut blend_alpha = None;
            device
                .CreateBlendState(
                    &D3D11_BLEND_DESC {
                        AlphaToCoverageEnable: BOOL(0),
                        IndependentBlendEnable: BOOL(0),
                        RenderTarget: [D3D11_RENDER_TARGET_BLEND_DESC {
                            BlendEnable: BOOL(1),
                            SrcBlend: D3D11_BLEND_SRC_ALPHA,
                            DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                            BlendOp: D3D11_BLEND_OP_ADD,
                            SrcBlendAlpha: D3D11_BLEND_ONE,
                            DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                            BlendOpAlpha: D3D11_BLEND_OP_ADD,
                            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as _,
                        }; 8],
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

                vertex_shader,
                pixel_shader,
                blend_alpha,
                sampler_state,
            })
        }
    }

    pub fn update_texture(&mut self, layer: u32, shared: UpdateSharedHandle) {
        if shared.handle.is_none() {
            self.textures.remove(&layer);
            return;
        }
        self.textures.entry(layer).or_default().update(shared);
    }

    #[tracing::instrument(skip(self))]
    pub fn draw(
        &mut self,
        device: &ID3D11Device,
        cx: &ID3D11DeviceContext,
        layer: u32,
        position: (i32, i32),
        size: (u32, u32),
        screen: (u32, u32),
    ) -> anyhow::Result<()> {
        if screen.0 == 0 || screen.1 == 0 {
            return Ok(());
        }

        let Some(Dx11Tex { view, mutex, .. }) = self
            .textures
            .entry(layer)
            .or_default()
            .get_or_create(|handle| open_shared_texture(device, handle))?
        else {
            return Ok(());
        };

        let rect = [
            (position.0 as f32 / screen.0 as f32) * 2.0 - 1.0,
            -(position.1 as f32 / screen.1 as f32) * 2.0 + 1.0,
            (size.0 as f32 / screen.0 as f32) * 2.0,
            -(size.1 as f32 / screen.1 as f32) * 2.0,
        ];

        unsafe {
            {
                let mut mapped_cbuffer = D3D11_MAPPED_SUBRESOURCE::default();
                cx.Map(
                    &self.constant_buffer,
                    0,
                    D3D11_MAP_WRITE_DISCARD,
                    0,
                    Some(&mut mapped_cbuffer),
                )?;
                mapped_cbuffer.pData.cast::<[f32; 4]>().write(rect);
                cx.Unmap(&self.constant_buffer, 0);
            }

            cx.OMSetBlendState(&self.blend_alpha, None, 0x00ffffff);
            cx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: screen.0 as _,
                Height: screen.1 as _,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));

            cx.VSSetShader(&self.vertex_shader, None);
            cx.PSSetShader(&self.pixel_shader, None);
            cx.PSSetSamplers(0, Some(&[Some(self.sampler_state.clone())]));
            cx.VSSetConstantBuffers(0, Some(&[Some(self.constant_buffer.clone())]));
            cx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);

            with_keyed_mutex(mutex.as_ref(), || {
                cx.PSSetShaderResources(0, Some(&[Some(view.clone())]));
                cx.Draw(4, 0);
            })?;
        }

        Ok(())
    }
}

fn open_shared_texture(
    device: &ID3D11Device,
    handle: NonZeroU32,
) -> anyhow::Result<Option<Dx11Tex>> {
    let texture = match glint_gpu_texture::open_shared_texture2d(device, handle.get()) {
        Ok(texture) => texture,
        Err(_) => return Ok(None),
    };

    let (width, height) = glint_gpu_texture::texture_dimensions(&texture);
    if width == 0 || height == 0 {
        return Ok(None);
    }

    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        texture.GetDesc(&mut desc);
    }

    let mutex = texture.cast::<IDXGIKeyedMutex>().ok();
    let mut view = None;
    unsafe {
        device.CreateShaderResourceView(
            &texture,
            Some(&D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: desc.Format,
                ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                    },
                },
            }),
            Some(&mut view),
        )?;
    }
    let view = view.context("cannot create texture view")?;

    Ok(Some(Dx11Tex { mutex, view }))
}
