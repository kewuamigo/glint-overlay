mod sync;

use anyhow::Context;
use core::{
    mem::ManuallyDrop,
    slice::{self},
};
use glint_overlay_common::request::UpdateSharedHandle;
use std::collections::HashMap;
use sync::RendererFence;
use windows::{
    Win32::{
        Foundation::{HANDLE, RECT},
        Graphics::{
            Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP,
            Direct3D11::ID3D11Device,
            Direct3D12::*,
            Dxgi::{
                Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC},
                IDXGIKeyedMutex, IDXGISwapChain, IDXGISwapChain3,
            },
        },
    },
    core::{BOOL, Interface},
};

use crate::{
    hook::util::original_execute_command_lists,
    renderer::{dx::shaders, hdr::OverlayBlitColorSpace},
    texture::OverlayTextureState,
    util::{MailboxSample, mailbox_sample, with_keyed_mutex_sampled, wrap_com_manually_drop},
};

/// DX12 blit source after AcquireSync(0) on the interop keyed mutex.
/// Lock → shared + last-good snapshot. Miss+cache → last-good. Miss+no cache → skip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dx12MailboxAction {
    Skip,
    DrawLastGood,
    DrawSharedAndSnapshot,
}

fn dx12_mailbox_action(lock_held: bool, has_cache: bool) -> Dx12MailboxAction {
    match mailbox_sample(lock_held, has_cache) {
        MailboxSample::Skip => Dx12MailboxAction::Skip,
        MailboxSample::Cache => Dx12MailboxAction::DrawLastGood,
        MailboxSample::Live => Dx12MailboxAction::DrawSharedAndSnapshot,
    }
}

const RENDER_TARGET_BLEND_DESC: D3D12_RENDER_TARGET_BLEND_DESC = D3D12_RENDER_TARGET_BLEND_DESC {
    BlendEnable: BOOL(1),
    SrcBlend: D3D12_BLEND_SRC_ALPHA,
    DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
    BlendOp: D3D12_BLEND_OP_ADD,
    SrcBlendAlpha: D3D12_BLEND_ONE,
    DestBlendAlpha: D3D12_BLEND_INV_SRC_ALPHA,
    BlendOpAlpha: D3D12_BLEND_OP_ADD,
    RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as _,
    LogicOpEnable: BOOL(0),
    LogicOp: D3D12_LOGIC_OP_NOOP,
};

const SAMPLER: D3D12_STATIC_SAMPLER_DESC = D3D12_STATIC_SAMPLER_DESC {
    Filter: D3D12_FILTER_MIN_MAG_MIP_POINT,
    AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
    AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
    AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
    MipLODBias: 0.0,
    MaxAnisotropy: 0,
    ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
    BorderColor: D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
    MinLOD: 0.0,
    MaxLOD: D3D12_FLOAT32_MAX,
    ShaderRegister: 0,
    RegisterSpace: 0,
    ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
};

#[inline]
fn root_sig() -> D3D12_ROOT_SIGNATURE_DESC {
    D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: 2,
        pParameters: [
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Constants: D3D12_ROOT_CONSTANTS {
                        ShaderRegister: 0,
                        RegisterSpace: 0,
                        Num32BitValues: 4,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_VERTEX,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                        NumDescriptorRanges: 1,
                        pDescriptorRanges: &D3D12_DESCRIPTOR_RANGE {
                            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
                            NumDescriptors: 1,
                            BaseShaderRegister: 0,
                            RegisterSpace: 0,
                            OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
                        },
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            },
        ]
        .as_ptr() as _,
        NumStaticSamplers: 1,
        pStaticSamplers: &SAMPLER,
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT
            | D3D12_ROOT_SIGNATURE_FLAG_DENY_HULL_SHADER_ROOT_ACCESS
            | D3D12_ROOT_SIGNATURE_FLAG_DENY_DOMAIN_SHADER_ROOT_ACCESS
            | D3D12_ROOT_SIGNATURE_FLAG_DENY_GEOMETRY_SHADER_ROOT_ACCESS,
    }
}

const RASTERIZER_STATE: D3D12_RASTERIZER_DESC = D3D12_RASTERIZER_DESC {
    FillMode: D3D12_FILL_MODE_SOLID,
    CullMode: D3D12_CULL_MODE_NONE,
    FrontCounterClockwise: BOOL(0),
    DepthBias: D3D12_DEFAULT_DEPTH_BIAS,
    DepthBiasClamp: D3D12_DEFAULT_DEPTH_BIAS_CLAMP,
    SlopeScaledDepthBias: D3D12_DEFAULT_SLOPE_SCALED_DEPTH_BIAS,
    DepthClipEnable: BOOL(0),
    MultisampleEnable: BOOL(0),
    AntialiasedLineEnable: BOOL(0),
    ForcedSampleCount: 0,
    ConservativeRaster: D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
};

const MAX_RENDER_TARGETS: usize = D3D12_SIMULTANEOUS_RENDER_TARGET_COUNT as _;
/// One SRV slot per DXGI overlay layer (0 = Electron, 1+ = CEF, …).
const MAX_LAYER_SRVS: u32 = 8;

struct Dx12Tex {
    resource: ID3D12Resource,
    lock: Dx12Lock,
    /// Explicit PSR after a Live copy+Draw. Does not decay; next copy needs PSR → COPY_SOURCE.
    left_in_psr: bool,
}

#[derive(Clone)]
enum Dx12Lock {
    /// Mailbox NT handle could not be opened on `RenderData.interop`.
    InteropFailed,
    NoMutex,
    Mutex(IDXGIKeyedMutex),
}

struct LastGood {
    resource: ID3D12Resource,
    width: u64,
    height: u32,
    format: DXGI_FORMAT,
    srv_ready: bool,
}

pub struct Dx12Renderer {
    sig: ID3D12RootSignature,

    /// Alpha blend over the game (L0 Electron and L1 CEF with transparent outside).
    pipeline: ID3D12PipelineState,
    pipeline_scrgb: ID3D12PipelineState,
    pipeline_pq: ID3D12PipelineState,
    /// Per-layer shared-texture cache (update only when handle changes).
    textures: HashMap<u32, OverlayTextureState<Dx12Tex>>,
    /// Private D3D12 snapshot; CopyResource while the interop mutex is held.
    last_good: HashMap<u32, LastGood>,
    texture_descriptor: ID3D12DescriptorHeap,
    srv_descriptor_size: usize,

    command_list: [(ID3D12GraphicsCommandList, ID3D12CommandAllocator); MAX_RENDER_TARGETS],
    fence: RendererFence,
    /// Open command list for this Present (`backbuffer_index`, resource). One Execute/Signal for all layers.
    recording: Option<(u32, ID3D12Resource)>,
    bb_state: [u32; MAX_RENDER_TARGETS],
}

impl Dx12Renderer {
    #[tracing::instrument]
    pub fn new(device: &ID3D12Device, swapchain: &IDXGISwapChain) -> anyhow::Result<Self> {
        unsafe {
            let swapchain_desc = swapchain.GetDesc()?;

            let mut sig = None;
            D3D12SerializeRootSignature(&root_sig(), D3D_ROOT_SIGNATURE_VERSION_1, &mut sig, None)?;
            let sig = sig.context("cannot create dx12 root signature")?;
            let sig = device.CreateRootSignature::<ID3D12RootSignature>(
                0,
                slice::from_raw_parts(sig.GetBufferPointer().cast::<u8>(), sig.GetBufferSize()),
            )?;

            let mut pipeline_desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                pRootSignature: wrap_com_manually_drop(&sig),
                VS: D3D12_SHADER_BYTECODE {
                    pShaderBytecode: shaders::VERTEX_SHADER.as_ptr().cast(),
                    BytecodeLength: shaders::VERTEX_SHADER.len(),
                },
                PS: D3D12_SHADER_BYTECODE {
                    pShaderBytecode: shaders::PIXEL_SHADER.as_ptr().cast(),
                    BytecodeLength: shaders::PIXEL_SHADER.len(),
                },
                BlendState: D3D12_BLEND_DESC {
                    AlphaToCoverageEnable: BOOL(0),
                    IndependentBlendEnable: BOOL(0),
                    RenderTarget: [RENDER_TARGET_BLEND_DESC; 8],
                },
                RasterizerState: RASTERIZER_STATE,
                PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                NumRenderTargets: 1,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                SampleMask: u32::MAX,
                ..Default::default()
            };
            pipeline_desc.RTVFormats[0] = swapchain_desc.BufferDesc.Format;

            let mut make_pso = |ps: &[u8]| -> anyhow::Result<ID3D12PipelineState> {
                pipeline_desc.PS = D3D12_SHADER_BYTECODE {
                    pShaderBytecode: ps.as_ptr().cast(),
                    BytecodeLength: ps.len(),
                };
                Ok(device.CreateGraphicsPipelineState::<ID3D12PipelineState>(&pipeline_desc)?)
            };
            let pipeline = make_pso(shaders::PIXEL_SHADER)?;
            let pipeline_scrgb = make_pso(shaders::PIXEL_SHADER_SCRGB)?;
            let pipeline_pq = make_pso(shaders::PIXEL_SHADER_PQ)?;

            let command_list = array_util::try_from_fn(|_| {
                let command_alloc = device.CreateCommandAllocator::<ID3D12CommandAllocator>(
                    D3D12_COMMAND_LIST_TYPE_DIRECT,
                )?;

                let command_list = device.CreateCommandList::<_, _, ID3D12GraphicsCommandList>(
                    0,
                    D3D12_COMMAND_LIST_TYPE_DIRECT,
                    &command_alloc,
                    None,
                )?;
                command_list.Close()?;

                Ok::<_, anyhow::Error>((command_list, command_alloc))
            })?;

            let texture_descriptor = device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(
                &D3D12_DESCRIPTOR_HEAP_DESC {
                    NumDescriptors: MAX_LAYER_SRVS,
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    ..Default::default()
                },
            )?;
            let srv_descriptor_size = device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
                as usize;

            Ok(Self {
                sig,

                pipeline,
                pipeline_scrgb,
                pipeline_pq,
                textures: HashMap::new(),
                last_good: HashMap::new(),
                texture_descriptor,
                srv_descriptor_size,

                command_list,
                fence: RendererFence::new(device)?,
                recording: None,
                bb_state: [0; MAX_RENDER_TARGETS],
            })
        }
    }

    pub fn update_texture(&mut self, layer: u32, shared: UpdateSharedHandle) {
        if self.fence.wait_pending().is_err() {
            return;
        }
        if shared.handle.is_none() {
            self.textures.remove(&layer);
            self.last_good.remove(&layer);
            return;
        }
        self.textures.entry(layer).or_default().update(shared);
    }

    #[tracing::instrument(skip(self))]
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &ID3D12Device,
        interop: &ID3D11Device,
        swapchain: &IDXGISwapChain3,
        backbuffer_index: u32,
        render_target: D3D12_CPU_DESCRIPTOR_HANDLE,
        layer: u32,
        position: (i32, i32),
        size: (u32, u32),
        screen: (u32, u32),
        color_space: OverlayBlitColorSpace,
    ) -> anyhow::Result<()> {
        if screen.0 == 0 || screen.1 == 0 {
            return Ok(());
        }

        if self.recording.is_none() {
            self.fence.wait_pending()?;
        }

        let (shared, lock) = {
            let Some(tex) = self
                .textures
                .entry(layer)
                .or_default()
                .get_or_create(|handle| open_shared(device, interop, handle))?
            else {
                return Ok(());
            };
            (tex.resource.clone(), tex.lock.clone())
        };

        let has_cache = self.last_good.contains_key(&layer);
        let rect: [f32; 4] = [
            (position.0 as f32 / screen.0 as f32) * 2.0 - 1.0,
            -(position.1 as f32 / screen.1 as f32) * 2.0 + 1.0,
            (size.0 as f32 / screen.0 as f32) * 2.0,
            -(size.1 as f32 / screen.1 as f32) * 2.0,
        ];

        let sampled = with_dx12_lock(&lock, |lock_held| -> anyhow::Result<()> {
            let action = dx12_mailbox_action(lock_held, has_cache);
            let src = match action {
                Dx12MailboxAction::Skip => return Ok(()),
                Dx12MailboxAction::DrawLastGood => {
                    let Some(good) = self.last_good.get(&layer) else {
                        return Ok(());
                    };
                    good.resource.clone()
                }
                Dx12MailboxAction::DrawSharedAndSnapshot => shared.clone(),
            };

            let slot = (layer.min(MAX_LAYER_SRVS - 1)) as usize;
            let (cpu_srv, gpu_srv) = unsafe {
                let cpu = self.texture_descriptor.GetCPUDescriptorHandleForHeapStart();
                let gpu = self.texture_descriptor.GetGPUDescriptorHandleForHeapStart();
                (
                    D3D12_CPU_DESCRIPTOR_HANDLE {
                        ptr: cpu.ptr + self.srv_descriptor_size * slot,
                    },
                    D3D12_GPU_DESCRIPTOR_HANDLE {
                        ptr: gpu.ptr + (self.srv_descriptor_size * slot) as u64,
                    },
                )
            };
            bind_srv(device, &src, cpu_srv);

            unsafe {
                let backbuffer = swapchain.GetBuffer::<ID3D12Resource>(backbuffer_index)?;
                let (ref command_list, ref command_alloc) =
                    self.command_list[backbuffer_index as usize];

                if self.recording.is_none() {
                    command_alloc.Reset()?;
                    command_list.Reset(command_alloc, self.pipeline(color_space))?;
                    command_list.SetGraphicsRootSignature(&self.sig);
                    command_list.SetDescriptorHeaps(&[Some(self.texture_descriptor.clone())]);
                    command_list.RSSetViewports(&[D3D12_VIEWPORT {
                        TopLeftX: 0.0,
                        TopLeftY: 0.0,
                        Width: screen.0 as _,
                        Height: screen.1 as _,
                        MinDepth: D3D12_MIN_DEPTH,
                        MaxDepth: D3D12_MAX_DEPTH,
                    }]);
                    command_list.RSSetScissorRects(&[RECT {
                        left: 0,
                        top: 0,
                        right: screen.0 as _,
                        bottom: screen.1 as _,
                    }]);
                    let stored = self.bb_state[backbuffer_index as usize];
                    let after = D3D12_RESOURCE_STATE_RENDER_TARGET.0 as u32;
                    if resource_barrier_needed(stored, after) {
                        command_list.ResourceBarrier(&[transition(
                            &backbuffer,
                            D3D12_RESOURCE_STATES(stored as i32),
                            D3D12_RESOURCE_STATE_RENDER_TARGET,
                        )]);
                        self.bb_state[backbuffer_index as usize] = after;
                    }
                    command_list.OMSetRenderTargets(1, Some(&render_target), true, None);
                    command_list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
                    self.recording = Some((backbuffer_index, backbuffer));
                }

                if action == Dx12MailboxAction::DrawSharedAndSnapshot {
                    let src_in_psr = match self.textures.get_mut(&layer) {
                        Some(
                            OverlayTextureState::Created(_, tex)
                            | OverlayTextureState::CreatedPending(_, tex, _),
                        ) => &mut tex.left_in_psr,
                        _ => return Ok(()),
                    };
                    snapshot_last_good(
                        device,
                        command_list,
                        &mut self.last_good,
                        layer,
                        &shared,
                        src_in_psr,
                    );
                }

                command_list.SetGraphicsRoot32BitConstants(0, 4, rect.as_ptr().cast(), 0);
                command_list.SetGraphicsRootDescriptorTable(1, gpu_srv);
                command_list.DrawInstanced(4, 1, 0, 0);
            }
            Ok(())
        })
        .map_err(anyhow::Error::from)?;
        match sampled {
            Some(Ok(())) | None => Ok(()),
            Some(Err(e)) => Err(e),
        }
    }

    pub fn finish(&mut self, queue: &ID3D12CommandQueue) -> anyhow::Result<()> {
        let Some((backbuffer_index, backbuffer)) = self.recording.take() else {
            return Ok(());
        };
        unsafe {
            let (ref command_list, _) = self.command_list[backbuffer_index as usize];
            command_list.ResourceBarrier(&[transition(
                &backbuffer,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_PRESENT,
            )]);
            self.bb_state[backbuffer_index as usize] = D3D12_RESOURCE_STATE_PRESENT.0 as u32;
            command_list.Close()?;
            original_execute_command_lists(queue, &[Some(command_list.clone().into())]);
        }
        self.fence.register(queue)?;
        Ok(())
    }

    fn pipeline(&self, color_space: OverlayBlitColorSpace) -> &ID3D12PipelineState {
        match color_space {
            OverlayBlitColorSpace::Sdr => &self.pipeline,
            OverlayBlitColorSpace::Scrgb => &self.pipeline_scrgb,
            OverlayBlitColorSpace::Pq => &self.pipeline_pq,
        }
    }
}

impl Drop for Dx12Renderer {
    fn drop(&mut self) {
        if !self.fence.is_pending() {
            return;
        }
        std::mem::forget(self.sig.clone());
        std::mem::forget(self.pipeline.clone());
        std::mem::forget(self.pipeline_scrgb.clone());
        std::mem::forget(self.pipeline_pq.clone());
        std::mem::forget(self.texture_descriptor.clone());
        for tex in self.textures.values() {
            if let OverlayTextureState::Created(_, res)
            | OverlayTextureState::CreatedPending(_, res, _) = tex
            {
                std::mem::forget(res.resource.clone());
            }
        }
        for good in self.last_good.values() {
            std::mem::forget(good.resource.clone());
        }
        for (list, alloc) in &self.command_list {
            std::mem::forget(list.clone());
            std::mem::forget(alloc.clone());
        }
        if let Some((_, bb)) = &self.recording {
            std::mem::forget(bb.clone());
        }
        self.fence.leak();
    }
}

unsafe impl Send for Dx12Renderer {}
unsafe impl Sync for Dx12Renderer {}

fn open_shared(
    device: &ID3D12Device,
    interop: &ID3D11Device,
    handle: core::num::NonZeroU32,
) -> anyhow::Result<Option<Dx12Tex>> {
    let mut texture = None;
    unsafe {
        device.OpenSharedHandle::<ID3D12Resource>(HANDLE(handle.get() as _), &mut texture)?;
    }
    let resource = texture.context("cannot open shared texture")?;
    let desc = unsafe { resource.GetDesc() };
    if desc.Width == 0 || desc.Height == 0 {
        return Ok(None);
    }
    let lock = match glint_gpu_texture::open_shared_texture2d(interop, handle.get()) {
        Ok(d3d11) => match d3d11.cast::<IDXGIKeyedMutex>() {
            Ok(mutex) => Dx12Lock::Mutex(mutex),
            Err(_) => Dx12Lock::NoMutex,
        },
        Err(_) => Dx12Lock::InteropFailed,
    };
    Ok(Some(Dx12Tex {
        resource,
        lock,
        left_in_psr: false,
    }))
}

fn with_dx12_lock<R>(
    lock: &Dx12Lock,
    f: impl FnOnce(bool) -> R,
) -> windows::core::Result<Option<R>> {
    match lock {
        Dx12Lock::InteropFailed => Ok(Some(f(false))),
        Dx12Lock::NoMutex => Ok(Some(f(true))),
        Dx12Lock::Mutex(mutex) => with_keyed_mutex_sampled(Some(mutex), f),
    }
}

fn bind_srv(device: &ID3D12Device, texture: &ID3D12Resource, cpu_srv: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let desc = unsafe { texture.GetDesc() };
    unsafe {
        device.CreateShaderResourceView(
            texture,
            Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Format: desc.Format,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV {
                        MipLevels: 1,
                        ..Default::default()
                    },
                },
            }),
            cpu_srv,
        );
    }
}

fn snapshot_last_good(
    device: &ID3D12Device,
    command_list: &ID3D12GraphicsCommandList,
    last_good: &mut HashMap<u32, LastGood>,
    layer: u32,
    src: &ID3D12Resource,
    src_in_psr: &mut bool,
) {
    let desc = unsafe { src.GetDesc() };
    if desc.Width == 0 || desc.Height == 0 {
        return;
    }
    let reuse = last_good.get(&layer).is_some_and(|g| {
        g.width == desc.Width && g.height == desc.Height && g.format == desc.Format
    });
    if !reuse {
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let copy_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: desc.Width,
            Height: desc.Height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: desc.Format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_NONE,
        };
        let mut resource = None;
        if unsafe {
            device.CreateCommittedResource::<ID3D12Resource>(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &copy_desc,
                D3D12_RESOURCE_STATE_COPY_DEST,
                None,
                &mut resource,
            )
        }
        .is_err()
        {
            return;
        }
        let Some(resource) = resource else {
            return;
        };
        last_good.insert(
            layer,
            LastGood {
                resource,
                width: desc.Width,
                height: desc.Height,
                format: desc.Format,
                srv_ready: false,
            },
        );
    }
    let Some(good) = last_good.get_mut(&layer) else {
        return;
    };
    unsafe {
        if good.srv_ready {
            command_list.ResourceBarrier(&[transition(
                &good.resource,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )]);
        }
        if *src_in_psr {
            command_list.ResourceBarrier(&[transition(
                src,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
        }
        command_list.CopyResource(&good.resource, src);
        command_list.ResourceBarrier(&[
            transition(
                &good.resource,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            ),
            transition(
                src,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            ),
        ]);
    }
    good.srv_ready = true;
    *src_in_psr = true;
}

fn resource_barrier_needed(stored: u32, after: u32) -> bool {
    stored != after
}

#[cfg(test)]
mod tests {
    use super::{Dx12MailboxAction, dx12_mailbox_action, resource_barrier_needed};

    #[test]
    fn busy_with_cache_uses_last_good_not_shared() {
        assert_eq!(
            dx12_mailbox_action(false, true),
            Dx12MailboxAction::DrawLastGood
        );
    }

    #[test]
    fn busy_without_cache_skips_quad() {
        assert_eq!(dx12_mailbox_action(false, false), Dx12MailboxAction::Skip);
    }

    #[test]
    fn lock_samples_shared_and_snapshots() {
        assert_eq!(
            dx12_mailbox_action(true, false),
            Dx12MailboxAction::DrawSharedAndSnapshot
        );
        assert_eq!(
            dx12_mailbox_action(true, true),
            Dx12MailboxAction::DrawSharedAndSnapshot
        );
    }

    #[test]
    fn resource_barrier_needed_skips_when_stored_matches() {
        assert!(!resource_barrier_needed(4, 4));
        assert!(!resource_barrier_needed(1024, 1024));
        assert!(!resource_barrier_needed(0, 0));
        assert!(!resource_barrier_needed(1, 1));
    }

    #[test]
    fn resource_barrier_needed_emits_when_differs() {
        assert!(resource_barrier_needed(0, 4));
        assert!(resource_barrier_needed(4, 0));
        assert!(resource_barrier_needed(1, 1024));
    }
}

unsafe fn transition(
    res: &ID3D12Resource,
    from: D3D12_RESOURCE_STATES,
    to: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: unsafe { wrap_com_manually_drop(res) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: from,
                StateAfter: to,
            }),
        },
    }
}
