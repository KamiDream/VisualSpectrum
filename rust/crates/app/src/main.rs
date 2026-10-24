//! VisualSpectrum 桌面端（Rust 原型）。
//!
//! 渲染管线：
//!
//! 1. **场景**：频谱条 + 散射粒子画到一张离屏纹理（透明底）；
//! 2. **余辉**：上一帧乘一个衰减系数铺底，条/粒子退下去后留下拖尾；
//! 3. **bloom**：场景降采样 + 两趟可分离高斯，得到 1/4 分辨率的辉光；
//! 4. **CRT 合成**：全屏 pass 做**桶形畸变 + RGB 分色**，叠上 bloom 和背景色，写进窗口。
//!
//! 还没做的：同心圆中心、无边框 / 置顶 / 鼠标穿透的浮层形态、HTTP 控制面板。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

mod single_instance;
mod tray;

use single_instance::{Acquire, SingleInstance};

use vs_audio::AudioEngine;
use vs_core::{Appearance, Config, ConfigStore};
use vs_scene::{palette, parse_hex, Scene, Viewport};

const MAX_BARS: usize = 512;
/// 和 `vs-scene` 里的粒子上限保持一致。
const MAX_PARTICLES: usize = 700;
/// 中心标签渐变最多支持的色标数 / 同心环数量。
const CENTER_STOPS: usize = 8;
const CENTER_RINGS: usize = 10;
const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// 桶形 / 分色拉满时的强度（调过的“足够明显但不糊”的档位）。
const BARREL_MAX: f32 = 0.35;
const ABERRATION_MAX: f32 = 0.0035;
const BLOOM_STRENGTH: f32 = 0.9;

/// 内部渲染分辨率倍数的手动覆盖（`VISUALSPECTRUM_SCALE`，方便调）。
/// 自适应策略：软件光栅器降到 0.5（像素量 1/4），硬件加速用全分辨率。
fn render_scale_override() -> Option<f32> {
    std::env::var("VISUALSPECTRUM_SCALE")
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .map(|v| v.clamp(0.2, 1.0))
}

fn is_software_adapter(info: &wgpu::AdapterInfo) -> bool {
    if matches!(info.device_type, wgpu::DeviceType::Cpu) {
        return true;
    }
    let name = info.name.to_lowercase();
    ["llvmpipe", "lavapipe", "swiftshader", "softpipe"]
        .iter()
        .any(|marker| name.contains(marker))
}

fn scaled_size(size: PhysicalSize<u32>, scale: f32) -> PhysicalSize<u32> {
    PhysicalSize::new(
        ((size.width as f32 * scale).round() as u32).max(1),
        ((size.height as f32 * scale).round() as u32).max(1),
    )
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BarInstance {
    p0: [f32; 2],
    p1: [f32; 2],
    width: f32,
    _pad: f32,
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewportUniform {
    size: [f32; 2],
    /// 余辉每帧的衰减系数（`exp(-dt/tau)`），0 表示关闭。
    decay: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PostUniforms {
    bg: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ParticleInstance {
    center: [f32; 2],
    radius: f32,
    alpha: f32,
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CenterUniforms {
    center: [f32; 2],
    radius: f32,
    label_radius: f32,
    spin: f32,
    line_width: f32,
    ring_inner: f32,
    ring_outer: f32,
    /// 低频能量（0~1），让中心轻微起伏。
    bass: f32,
    /// 光晕强度倍数（`center_glow / 100`）。
    glow: f32,
    stop_count: u32,
    /// 补齐到 48 字节：WGSL 里 `array<vec4f>` 要求 16 字节对齐，Rust 侧 `[[f32;4]; N]`
    /// 只有 4 字节对齐，所以显式补到 48。
    _pad: [u32; 1],
    stops: [[f32; 4]; CENTER_STOPS],
    ring_colors: [[f32; 4]; CENTER_RINGS],
}

fn create_texture(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SCENE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

struct RenderState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    /// 内部离屏纹理尺寸（比窗口小，CRT 时上采样）。
    render_size: PhysicalSize<u32>,
    render_scale: f32,

    bar_pipeline: wgpu::RenderPipeline,
    bar_uniform_buffer: wgpu::Buffer,
    bar_bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    /// 顶端连线的实例（每段两「层」：外晕 + 芯），最多 2 × MAX_BARS。
    link_instance_buffer: wgpu::Buffer,
    particle_pipeline: wgpu::RenderPipeline,
    particle_instance_buffer: wgpu::Buffer,
    center_pipeline: wgpu::RenderPipeline,
    center_uniform_buffer: wgpu::Buffer,
    center_bind_group: wgpu::BindGroup,

    bloom_h_pipeline: wgpu::RenderPipeline,
    bloom_v_pipeline: wgpu::RenderPipeline,
    bloom_bgl: wgpu::BindGroupLayout,
    fade_pipeline: wgpu::RenderPipeline,
    crt_pipeline: wgpu::RenderPipeline,
    crt_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    post_uniform_buffer: wgpu::Buffer,

    /// 场景双缓冲：余辉需要读上一帧、写当前帧，只能 ping-pong。
    _scene_textures: [wgpu::Texture; 2],
    scene_views: [wgpu::TextureView; 2],
    /// 当前帧要写入 / 采样的下标；每帧取反。
    scene_index: usize,
    /// `fade_bind_groups[i]` 读 `scene_views[i]`。
    fade_bind_groups: [wgpu::BindGroup; 2],
    /// `bloom_h_bind_groups[i]` 读 `scene_views[i]`。
    bloom_h_bind_groups: [wgpu::BindGroup; 2],
    _bloom_a_texture: wgpu::Texture,
    bloom_a_view: wgpu::TextureView,
    bloom_v_bind_group: wgpu::BindGroup,
    _bloom_b_texture: wgpu::Texture,
    bloom_b_view: wgpu::TextureView,
    /// `crt_bind_groups[i]` 读 `scene_views[i]`。
    crt_bind_groups: [wgpu::BindGroup; 2],

    scene: Scene,
    engine: AudioEngine,
    appearance: Appearance,
    /// 给 HTTP 的实时频谱（渲染线程写、配置服务读）。
    live: Arc<Mutex<vs_server::Live>>,
    last_frame: Instant,
    /// 帧间隔：`high_fps=false` 锁 60fps，`true` 锁 120fps。
    /// （不再用「不限帧」：AutoNoVsync 下会跑到 600+fps，白白烧 GPU。）
    frame_interval: Duration,
    last_present: Instant,
    /// 帧率统计（每秒写一次给 HTTP）。
    frames: u32,
    fps_window: Instant,
    /// 运行时开关（键盘 B / C）—— 方便对比效果、定位问题。
    bloom_on: bool,
    crt_on: bool,
}

impl RenderState {
    fn new(
        window: Arc<Window>,
        config: Config,
        live: Arc<Mutex<vs_server::Live>>,
        engine: Option<AudioEngine>,
    ) -> Result<Self, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window)
            .map_err(|err| format!("创建 surface 失败：{err}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|err| format!("找不到 GPU 适配器：{err}"))?;

        // 软件光栅器（lavapipe/llvmpipe）撑不住全分辨率：内部降到 0.5 倍，
        // 有硬件加速就用 1.0（`VISUALSPECTRUM_SCALE` 可覆盖）。
        let adapter_info = adapter.get_info();
        let render_scale =
            render_scale_override().unwrap_or(if is_software_adapter(&adapter_info) {
                0.5
            } else {
                1.0
            });
        let render_size = scaled_size(size, render_scale);
        eprintln!(
            "适配器：{}（{:?}）；surface {}×{}，内部渲染 {}×{}（scale {:.2}）",
            adapter_info.name,
            adapter_info.device_type,
            size.width,
            size.height,
            render_size.width,
            render_size.height,
            render_scale
        );

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vs-device"),
            ..Default::default()
        }))
        .map_err(|err| format!("创建 GPU 设备失败：{err}"))?;

        let mut surface_config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| "该 surface 不被此适配器支持".to_string())?;
        let caps = surface.get_capabilities(&adapter);
        if caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            surface_config.alpha_mode = wgpu::CompositeAlphaMode::PreMultiplied;
        }
        // `high_fps` = 高刷模式：自己锁 120fps，仍然跟垂直同步（不撕裂）。
        surface_config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &surface_config);
        let frame_interval = if config.render.high_fps {
            Duration::from_secs_f64(1.0 / 120.0)
        } else {
            Duration::from_secs_f64(1.0 / 60.0)
        };

        // ---- 条 ----
        let bar_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bars"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let bar_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bar_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bars"),
            bind_group_layouts: &[Some(&bar_bgl)],
            immediate_size: 0,
        });
        let bar_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<BarInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 8,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 16,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 24,
                    shader_location: 3,
                },
            ],
        };
        let bar_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bars"),
            layout: Some(&bar_layout),
            vertex: wgpu::VertexState {
                module: &bar_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(bar_vertex_layout)],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &bar_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let bar_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport"),
            size: std::mem::size_of::<ViewportUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bar_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport"),
            layout: &bar_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: bar_uniform_buffer.as_entire_binding(),
            }],
        });
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bar instances"),
            size: (MAX_BARS * std::mem::size_of::<BarInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // ---- 散射粒子（复用条的 uniform 布局） ----
        let particle_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particles"),
            source: wgpu::ShaderSource::Wgsl(include_str!("particle.wgsl").into()),
        });
        let particle_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ParticleInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 8,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 12,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 3,
                },
            ],
        };
        let particle_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("particles"),
            layout: Some(&bar_layout),
            vertex: wgpu::VertexState {
                module: &particle_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(particle_vertex_layout)],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &particle_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_FORMAT,
                    // 预乘：光点亮度受 alpha 约束，叠上余辉不会无限过曝。
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let particle_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle instances"),
            size: (MAX_PARTICLES * std::mem::size_of::<ParticleInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let link_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("link instances"),
            size: (MAX_BARS * 2 * std::mem::size_of::<BarInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // ---- 中心：同心环 + X 形油光 + 色标标签 ----
        let center_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("center"),
            source: wgpu::ShaderSource::Wgsl(include_str!("center.wgsl").into()),
        });
        let center_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("center"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let center_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("center"),
            bind_group_layouts: &[Some(&bar_bgl), Some(&center_bgl)],
            immediate_size: 0,
        });
        let center_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("center"),
            layout: Some(&center_layout),
            vertex: wgpu::VertexState {
                module: &center_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &center_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_FORMAT,
                    // 预乘：环/光晕的 alpha 跟亮度走，配合余辉不会无限累积；标签 alpha=1 不透明覆盖。
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let center_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("center"),
            size: std::mem::size_of::<CenterUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let center_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("center"),
            layout: &center_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: center_uniform_buffer.as_entire_binding(),
            }],
        });

        // ---- 后处理公共资源 ----
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = || wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };

        let bloom_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom"),
            entries: &[sampler_entry(), tex_entry(1)],
        });
        let crt_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("crt"),
            entries: &[
                sampler_entry(),
                tex_entry(1),
                tex_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let bloom_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bloom"),
            source: wgpu::ShaderSource::Wgsl(include_str!("bloom.wgsl").into()),
        });
        let crt_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("crt"),
            source: wgpu::ShaderSource::Wgsl(include_str!("crt.wgsl").into()),
        });

        let bloom_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bloom"),
            bind_group_layouts: &[Some(&bloom_bgl)],
            immediate_size: 0,
        });
        let make_bloom_pipeline = |entry: &'static str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&bloom_layout),
                vertex: wgpu::VertexState {
                    module: &bloom_shader,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &bloom_shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: SCENE_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let bloom_h_pipeline = make_bloom_pipeline("fs_blur_h");
        let bloom_v_pipeline = make_bloom_pipeline("fs_blur_v");

        // ---- 余辉：group0 复用条的 uniform（含 decay），group1 复用 bloom 的 sampler+texture ----
        let afterglow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("afterglow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("afterglow.wgsl").into()),
        });
        let fade_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("afterglow"),
            bind_group_layouts: &[Some(&bar_bgl), Some(&bloom_bgl)],
            immediate_size: 0,
        });
        let fade_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("afterglow"),
            layout: Some(&fade_layout),
            vertex: wgpu::VertexState {
                module: &afterglow_shader,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &afterglow_shader,
                entry_point: Some("fs_fade"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let crt_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("crt"),
            bind_group_layouts: &[Some(&crt_bgl)],
            immediate_size: 0,
        });
        let crt_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("crt"),
            layout: Some(&crt_layout),
            vertex: wgpu::VertexState {
                module: &crt_shader,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &crt_shader,
                entry_point: Some("fs_crt"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let post_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post"),
            size: std::mem::size_of::<PostUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // ---- 离屏纹理 + 绑定组 ----
        let (scene_a_texture, scene_a_view) =
            create_texture(&device, "scene a", render_size.width, render_size.height);
        let (scene_b_texture, scene_b_view) =
            create_texture(&device, "scene b", render_size.width, render_size.height);
        let scene_views = [scene_a_view, scene_b_view];
        let bw = (render_size.width / 4).max(1);
        let bh = (render_size.height / 4).max(1);
        let (bloom_a_texture, bloom_a_view) = create_texture(&device, "bloom a", bw, bh);
        let (bloom_b_texture, bloom_b_view) = create_texture(&device, "bloom b", bw, bh);

        // sampler + texture 的绑定组（fade / bloom H 共用同一套 layout）。
        let textured_bind = |label: &str, view: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &bloom_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                ],
            })
        };
        let fade_bind_groups = [
            textured_bind("fade a", &scene_views[0]),
            textured_bind("fade b", &scene_views[1]),
        ];
        let bloom_h_bind_groups = [
            textured_bind("bloom h a", &scene_views[0]),
            textured_bind("bloom h b", &scene_views[1]),
        ];
        let bloom_v_bind_group = textured_bind("bloom v", &bloom_a_view);

        let make_crt_bind = |label: &str, view: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &crt_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&bloom_b_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: post_uniform_buffer.as_entire_binding(),
                    },
                ],
            })
        };
        let crt_bind_groups = [
            make_crt_bind("crt a", &scene_views[0]),
            make_crt_bind("crt b", &scene_views[1]),
        ];

        let appearance = config.appearance.clone();
        let crt_on = appearance.crt;
        let count = appearance.count.clamp(4, MAX_BARS as i64) as usize;
        // 窗口可能只是被“关掉”（销毁）过，音频引擎要留着 —— 否则每次唤回都会重开捕获。
        let engine = match engine {
            Some(mut engine) => {
                engine.reconfigure(count, appearance.gains());
                engine
            }
            None => {
                let mut engine = AudioEngine::new(count, &config.audio, appearance.gains());
                engine.start();
                engine
            }
        };

        Ok(Self {
            surface,
            device,
            queue,
            surface_config,
            size,
            render_size,
            render_scale,
            bar_pipeline,
            bar_uniform_buffer,
            bar_bind_group,
            instance_buffer,
            link_instance_buffer,
            particle_pipeline,
            particle_instance_buffer,
            center_pipeline,
            center_uniform_buffer,
            center_bind_group,
            bloom_h_pipeline,
            bloom_v_pipeline,
            bloom_bgl,
            fade_pipeline,
            crt_pipeline,
            crt_bgl,
            sampler,
            post_uniform_buffer,
            _scene_textures: [scene_a_texture, scene_b_texture],
            scene_views,
            scene_index: 0,
            fade_bind_groups,
            bloom_h_bind_groups,
            _bloom_a_texture: bloom_a_texture,
            bloom_a_view,
            bloom_v_bind_group,
            _bloom_b_texture: bloom_b_texture,
            bloom_b_view,
            crt_bind_groups,
            scene: Scene::new(0x5EED_2026),
            engine,
            appearance,
            live,
            last_frame: Instant::now(),
            frame_interval,
            last_present: Instant::now(),
            frames: 0,
            fps_window: Instant::now(),
            bloom_on: true,
            crt_on,
        })
    }

    /// 运行中切 `render.high_fps`（保存配置后走到这里）。
    fn set_high_fps(&mut self, high_fps: bool) {
        self.frame_interval = if high_fps {
            Duration::from_secs_f64(1.0 / 120.0)
        } else {
            Duration::from_secs_f64(1.0 / 60.0)
        };
        eprintln!("帧率上限 = {}fps", if high_fps { 120 } else { 60 });
    }

    /// 现在是否到了下一帧的时间（`high_fps` 时间隔为 0，永远就绪）。
    fn ready_for_frame(&self) -> bool {
        self.frame_interval.is_zero() || self.last_present.elapsed() >= self.frame_interval
    }

    /// 下一帧应该醒来的时刻（给事件循环排定时器用）。
    fn next_frame_deadline(&self) -> Instant {
        self.last_present + self.frame_interval
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.size = size;
        self.render_size = scaled_size(size, self.render_scale);
        self.surface_config.width = size.width;
        self.surface_config.height = size.height;
        self.surface.configure(&self.device, &self.surface_config);

        // 离屏纹理 / 绑定组跟着窗口大小重建（场景双缓冲 + bloom a/b）。
        let (scene_a_texture, scene_a_view) = create_texture(
            &self.device,
            "scene a",
            self.render_size.width,
            self.render_size.height,
        );
        let (scene_b_texture, scene_b_view) = create_texture(
            &self.device,
            "scene b",
            self.render_size.width,
            self.render_size.height,
        );
        let scene_views = [scene_a_view, scene_b_view];
        let bw = (self.render_size.width / 4).max(1);
        let bh = (self.render_size.height / 4).max(1);
        let (bloom_a_texture, bloom_a_view) = create_texture(&self.device, "bloom a", bw, bh);
        let (bloom_b_texture, bloom_b_view) = create_texture(&self.device, "bloom b", bw, bh);

        let bloom_bind = |label: &str, view: &wgpu::TextureView| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.bloom_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                ],
            })
        };
        let fade_bind_groups = [
            bloom_bind("fade a", &scene_views[0]),
            bloom_bind("fade b", &scene_views[1]),
        ];
        let bloom_h_bind_groups = [
            bloom_bind("bloom h a", &scene_views[0]),
            bloom_bind("bloom h b", &scene_views[1]),
        ];
        let bloom_v_bind_group = bloom_bind("bloom v", &bloom_a_view);

        let make_crt_bind = |label: &str, view: &wgpu::TextureView| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.crt_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&bloom_b_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.post_uniform_buffer.as_entire_binding(),
                    },
                ],
            })
        };
        let crt_bind_groups = [
            make_crt_bind("crt a", &scene_views[0]),
            make_crt_bind("crt b", &scene_views[1]),
        ];

        self._scene_textures = [scene_a_texture, scene_b_texture];
        self.scene_views = scene_views;
        self.fade_bind_groups = fade_bind_groups;
        self.bloom_h_bind_groups = bloom_h_bind_groups;
        self.bloom_v_bind_group = bloom_v_bind_group;
        self.crt_bind_groups = crt_bind_groups;
        self._bloom_a_texture = bloom_a_texture;
        self.bloom_a_view = bloom_a_view;
        self._bloom_b_texture = bloom_b_texture;
        self.bloom_b_view = bloom_b_view;
    }

    fn render(&mut self) {
        // 限帧：不到间隔就直接返回（GPU 不干活，事件循环空转代价极小）。
        if self.last_present.elapsed() < self.frame_interval {
            return;
        }
        let now = Instant::now();
        self.last_present = now;
        let dt = (now - self.last_frame).as_secs_f64().min(0.05);
        self.last_frame = now;

        let snapshot = self.engine.snapshot();
        let viewport = Viewport {
            width: self.render_size.width.max(1) as f64,
            height: self.render_size.height.max(1) as f64,
        };
        self.scene
            .update(dt, viewport, &snapshot.bands, &self.appearance);

        if let Ok(mut live) = self.live.lock() {
            live.bands = snapshot.bands.clone();
            live.live = snapshot.live;
            live.source = snapshot.source.clone();
            live.running = self.engine.running();
        }

        // 每秒统计一次真实帧率，供 /api/meta 查看。
        self.frames += 1;
        let window = self.fps_window.elapsed();
        if window >= Duration::from_secs(1) {
            let fps = f64::from(self.frames) / window.as_secs_f64();
            self.frames = 0;
            self.fps_window = Instant::now();
            if let Ok(mut live) = self.live.lock() {
                live.fps = fps;
            }
        }

        let afterglow = self.appearance.afterglow.clamp(0, 100) as f32;
        // 指数衰减（帧率无关）：50 → τ≈0.36s，100 → τ≈1.26s。
        let decay = if afterglow > 0.0 {
            let tau = 0.06 + 1.2 * (afterglow / 100.0).powi(2);
            (-(dt as f32) / tau).exp()
        } else {
            0.0
        };
        let viewport_uniform = ViewportUniform {
            size: [viewport.width as f32, viewport.height as f32],
            decay,
            _pad: 0.0,
        };
        self.queue.write_buffer(
            &self.bar_uniform_buffer,
            0,
            bytemuck::bytes_of(&viewport_uniform),
        );

        let bg = clear_color(&self.appearance);
        let (barrel, aberration) = if self.crt_on {
            (
                (self.appearance.barrel as f32 / 100.0) * BARREL_MAX,
                (self.appearance.aberration as f32 / 100.0) * ABERRATION_MAX,
            )
        } else {
            (0.0, 0.0)
        };
        let bloom_strength = if self.bloom_on { BLOOM_STRENGTH } else { 0.0 };
        let post = PostUniforms {
            bg: [bg.r as f32, bg.g as f32, bg.b as f32, bg.a as f32],
            params: [barrel, aberration, bloom_strength, 0.0],
        };
        self.queue
            .write_buffer(&self.post_uniform_buffer, 0, bytemuck::bytes_of(&post));

        let bars = self.scene.bars();
        let count = bars.len().min(MAX_BARS);
        let mut instances = Vec::with_capacity(count);
        for bar in bars.iter().take(count) {
            instances.push(BarInstance {
                p0: [bar.x0 as f32, bar.y0 as f32],
                p1: [bar.x1 as f32, bar.y1 as f32],
                width: bar.width as f32,
                _pad: 0.0,
                color: [
                    (bar.color[0] / 255.0) as f32,
                    (bar.color[1] / 255.0) as f32,
                    (bar.color[2] / 255.0) as f32,
                    1.0,
                ],
            });
        }
        if !instances.is_empty() {
            self.queue
                .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instances));
        }

        // 顶端连线：每段两层（3.2px 外晕 + 1.2px 芯），颜色由场景按调色板给。
        let links = self.scene.links();
        let lcount = links.len().min(MAX_BARS);
        let mut link_instances = Vec::with_capacity(lcount * 2);
        for link in links.iter().take(lcount) {
            let rgb = [
                (link.color[0] / 255.0) as f32,
                (link.color[1] / 255.0) as f32,
                (link.color[2] / 255.0) as f32,
            ];
            let p0 = [link.x0 as f32, link.y0 as f32];
            let p1 = [link.x1 as f32, link.y1 as f32];
            link_instances.push(BarInstance {
                p0,
                p1,
                width: (link.width * 2.667) as f32,
                _pad: 0.0,
                color: [rgb[0], rgb[1], rgb[2], 0.16],
            });
            link_instances.push(BarInstance {
                p0,
                p1,
                width: link.width as f32,
                _pad: 0.0,
                color: [rgb[0], rgb[1], rgb[2], 0.5],
            });
        }
        if !link_instances.is_empty() {
            self.queue.write_buffer(
                &self.link_instance_buffer,
                0,
                bytemuck::cast_slice(&link_instances),
            );
        }

        // 粒子：场景里的极坐标 → 屏幕像素，生命越接近尾端越暗。
        let particles = self.scene.particles();
        let pcount = particles.len().min(MAX_PARTICLES);
        let mut pinstances = Vec::with_capacity(pcount);
        for p in particles.iter().take(pcount) {
            let (x, y) = p.position(viewport);
            pinstances.push(ParticleInstance {
                center: [x as f32, y as f32],
                radius: p.size as f32,
                alpha: p.fade().powf(1.5) as f32,
                color: [
                    (p.color[0] / 255.0) as f32,
                    (p.color[1] / 255.0) as f32,
                    (p.color[2] / 255.0) as f32,
                    1.0,
                ],
            });
        }
        if !pinstances.is_empty() {
            self.queue.write_buffer(
                &self.particle_instance_buffer,
                0,
                bytemuck::cast_slice(&pinstances),
            );
        }

        // 中心：半径跟着低频轻微起伏；环色和标签色都走用户的调色板。
        let geom = self.scene.geometry();
        let shown = self.scene.shown();
        let bass = if shown.is_empty() {
            0.0
        } else {
            shown.iter().take(4).sum::<f64>() / shown.len().min(4) as f64
        };
        let center_radius = geom.radius_base * 0.92 * (1.0 + bass * 0.03);
        let label_radius = center_radius * 0.36;

        let mut ring_colors = [[0.0f32; 4]; CENTER_RINGS];
        for (slot, c) in ring_colors
            .iter_mut()
            .zip(palette(&self.appearance, CENTER_RINGS).iter())
        {
            *slot = [
                (c[0] / 255.0) as f32,
                (c[1] / 255.0) as f32,
                (c[2] / 255.0) as f32,
                1.0,
            ];
        }

        let mut stops = [[0.0f32; 4]; CENTER_STOPS];
        let mut stop_count = 0u32;
        for (i, hex) in self.appearance.colors.iter().take(CENTER_STOPS).enumerate() {
            let c = parse_hex(hex);
            stops[i] = [
                (c[0] / 255.0) as f32,
                (c[1] / 255.0) as f32,
                (c[2] / 255.0) as f32,
                1.0,
            ];
            stop_count = i as u32 + 1;
        }
        if stop_count == 0 {
            stops[0] = [0.494, 0.906, 1.0, 1.0];
            stop_count = 1;
        }

        if center_radius > 2.0 {
            let center = CenterUniforms {
                center: [geom.cx as f32, geom.cy as f32],
                radius: center_radius as f32,
                label_radius: label_radius as f32,
                spin: self.scene.rotation() as f32,
                line_width: (center_radius * 0.012).max(1.2) as f32,
                ring_inner: (label_radius * 1.12) as f32,
                ring_outer: (center_radius * 0.99) as f32,
                bass: bass as f32,
                glow: self.appearance.center_glow.clamp(0, 200) as f32 / 100.0,
                stop_count,
                _pad: [0; 1],
                stops,
                ring_colors,
            };
            self.queue
                .write_buffer(&self.center_uniform_buffer, 0, bytemuck::bytes_of(&center));
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.surface_config);
                return;
            }
            _ => return,
        };
        let swapchain_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        let transparent = wgpu::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };

        // 1) 场景 → scene[cur]：先铺上一帧淡出后的余辉，再用新条覆盖
        let cur = self.scene_index;
        let prev = 1 - cur;
        {
            let attachment = Some(wgpu::RenderPassColorAttachment {
                view: &self.scene_views[cur],
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(transparent),
                    store: wgpu::StoreOp::Store,
                },
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[attachment],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if decay > 0.0 {
                pass.set_pipeline(&self.fade_pipeline);
                pass.set_bind_group(0, &self.bar_bind_group, &[]);
                pass.set_bind_group(1, &self.fade_bind_groups[prev], &[]);
                pass.draw(0..3, 0..1);
            }
            if center_radius > 2.0 {
                pass.set_pipeline(&self.center_pipeline);
                pass.set_bind_group(0, &self.bar_bind_group, &[]);
                pass.set_bind_group(1, &self.center_bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
            if count > 0 {
                pass.set_pipeline(&self.bar_pipeline);
                pass.set_bind_group(0, &self.bar_bind_group, &[]);
                pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
                pass.draw(0..6, 0..count as u32);
            }
            if lcount > 0 {
                pass.set_pipeline(&self.bar_pipeline);
                pass.set_bind_group(0, &self.bar_bind_group, &[]);
                pass.set_vertex_buffer(0, self.link_instance_buffer.slice(..));
                pass.draw(0..6, 0..(lcount * 2) as u32);
            }
            if pcount > 0 {
                pass.set_pipeline(&self.particle_pipeline);
                pass.set_bind_group(0, &self.bar_bind_group, &[]);
                pass.set_vertex_buffer(0, self.particle_instance_buffer.slice(..));
                pass.draw(0..6, 0..pcount as u32);
            }
        }

        // 2) bloom：scene --H--> a --V--> b（关掉时跳过，省两次全屏 pass）
        if self.bloom_on {
            for (label, pipeline, bind_group, view) in [
                (
                    "bloom h",
                    &self.bloom_h_pipeline,
                    &self.bloom_h_bind_groups[cur],
                    &self.bloom_a_view,
                ),
                (
                    "bloom v",
                    &self.bloom_v_pipeline,
                    &self.bloom_v_bind_group,
                    &self.bloom_b_view,
                ),
            ] {
                let attachment = Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(transparent),
                        store: wgpu::StoreOp::Store,
                    },
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(label),
                    color_attachments: &[attachment],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        // 3) CRT 合成 → 窗口
        {
            let attachment = Some(wgpu::RenderPassColorAttachment {
                view: &swapchain_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(transparent),
                    store: wgpu::StoreOp::Store,
                },
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("crt"),
                color_attachments: &[attachment],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.crt_pipeline);
            pass.set_bind_group(0, &self.crt_bind_groups[cur], &[]);
            pass.draw(0..3, 0..1);
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);

        // 本帧写的那张下一帧当历史读。
        self.scene_index = prev;
    }
}

struct App {
    window: Option<Arc<Window>>,
    state: Option<RenderState>,
    store: Arc<Mutex<ConfigStore>>,
    live: Arc<Mutex<vs_server::Live>>,
    overlay_visible: Arc<AtomicBool>,
    /// 托盘菜单里的「退出」置位；主循环看到后结束事件循环。
    quit: Arc<AtomicBool>,
    /// 托盘服务句柄（活着才有图标；只要不 drop 就行，不在别处读）。
    _tray: Option<tray::TrayHandle>,
    /// 窗口隐藏时把音频引擎留在这里，唤回时复用（不然会重开捕获）。
    engine: Option<AudioEngine>,
    instance: SingleInstance,
    last_config_check: Instant,
    /// 上一次已应用到渲染参数上的配置版本 —— HTTP 保存/外部改文件都会让它 +1。
    last_applied_revision: u64,
}

/// 背景色（**预乘 alpha**）：`bg_opacity = 0` 就是全透明。
fn clear_color(appearance: &Appearance) -> wgpu::Color {
    let alpha = (appearance.bg_opacity as f64).clamp(0.0, 100.0) / 100.0;
    if alpha <= 0.0 {
        return wgpu::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };
    }
    let [r, g, b] = vs_scene::parse_hex(&appearance.bg_color);
    wgpu::Color {
        r: r / 255.0 * alpha,
        g: g / 255.0 * alpha,
        b: b / 255.0 * alpha,
        a: alpha,
    }
}

impl App {
    /// 建一扇新窗口 + 渲染器。失败时返回并保持隐藏状态。
    ///
    /// Wayland（winit 0.30）没有"显示/隐藏"这回事，`set_visible` 是空实现 ——
    /// 所以"隐藏"只能靠销毁窗口，"再显示"就只能重建。
    fn open_window(&mut self, event_loop: &ActiveEventLoop) {
        let config = {
            let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            store.data().clone()
        };
        // 两种模式都是“桌面上的装饰”：无边框、鼠标穿透。
        // `top` / `wallpaper` 的置顶/置底只是给 WM 的提示（Wayland 下 winit 是空实现，
        // 需要合成器规则；X11 会真的生效）。
        let wallpaper = config.appearance.mode == "wallpaper";
        let attributes = Window::default_attributes()
            .with_title("VisualSpectrum")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0))
            .with_transparent(true)
            .with_decorations(false)
            .with_window_level(if wallpaper {
                WindowLevel::AlwaysOnBottom
            } else {
                WindowLevel::AlwaysOnTop
            });
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                eprintln!("创建窗口失败：{err}");
                return;
            }
        };
        // 鼠标穿透：Wayland 下是把 input region 置空（winit 已实现）。
        match window.set_cursor_hittest(false) {
            Ok(()) => eprintln!("窗口形态：{}（无边框 · 鼠标穿透）", config.appearance.mode),
            Err(err) => eprintln!("鼠标穿透设置失败：{err}"),
        }
        match RenderState::new(
            Arc::clone(&window),
            config,
            Arc::clone(&self.live),
            self.engine.take(),
        ) {
            Ok(state) => {
                self.window = Some(window);
                self.state = Some(state);
            }
            Err(err) => eprintln!("初始化失败：{err}"),
        }
    }

    /// 销毁窗口但**留住音频引擎** —— 唤回来时不用重开捕获，也不会卡一下。
    fn hide_window(&mut self) {
        if let Some(state) = self.state.take() {
            self.engine = Some(state.engine);
        }
        self.window = None;
        eprintln!("窗口已隐藏（进程仍在运行；再次运行 visualspectrum 或点配置页的显示按钮可唤回）");
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        self.open_window(event_loop);
        if self.window.is_none() {
            event_loop.exit();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                // 关窗口 = 隐藏：Wayland 上不能 set_visible，所以直接销毁窗口；
                // 真正的销毁统一放到 about_to_wait 里做（和 /api/overlay 同一条路）。
                self.overlay_visible.store(false, Ordering::Relaxed);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character(text) => {
                        if let Some(state) = &mut self.state {
                            match text.as_str() {
                                "b" => {
                                    state.bloom_on = !state.bloom_on;
                                    eprintln!("bloom = {}", state.bloom_on);
                                }
                                "c" => {
                                    state.crt_on = !state.crt_on;
                                    eprintln!("crt = {}", state.crt_on);
                                }
                                "r" => {
                                    state.appearance.rotate = !state.appearance.rotate;
                                    eprintln!("rotate = {}", state.appearance.rotate);
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(state) = &mut self.state {
                    state.resize(size);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(state) = &mut self.state {
                    state.render();
                }
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.quit.load(Ordering::Relaxed) {
            eprintln!("托盘请求退出");
            event_loop.exit();
            return;
        }

        if self.instance.take_show_request() {
            self.overlay_visible.store(true, Ordering::Relaxed);
            eprintln!("收到唤醒请求 —— 显示可视化");
        }
        // 每 0.5s 看一眼配置：外部改文件靠 mtime，HTTP 保存靠 revision（同一条通道）。
        if self.last_config_check.elapsed() >= std::time::Duration::from_millis(500) {
            self.last_config_check = Instant::now();
            let (reload_error, revision, config) = {
                let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
                let reload = store.reload_if_changed();
                let revision = store.revision();
                let config = if revision != self.last_applied_revision {
                    Some(store.data().clone())
                } else {
                    None
                };
                (reload.error, revision, config)
            };
            if let Some(error) = reload_error {
                eprintln!("配置重载失败：{error}（继续用旧配置）");
            }
            if let Some(config) = config {
                self.last_applied_revision = revision;
                if let Some(state) = &mut self.state {
                    state.appearance = config.appearance.clone();
                    state.crt_on = config.appearance.crt;
                    let count = config.appearance.count.clamp(4, MAX_BARS as i64) as usize;
                    let gains = config.appearance.gains();
                    state.engine.reconfigure(count, gains);
                    state.engine.apply_settings(&config.audio, gains);
                    state.set_high_fps(config.render.high_fps);
                } else if let Some(engine) = &mut self.engine {
                    // 窗口隐藏着也要让音频设置生效（引擎被暂存在 App 里）。
                    let count = config.appearance.count.clamp(4, MAX_BARS as i64) as usize;
                    let gains = config.appearance.gains();
                    engine.reconfigure(count, gains);
                    engine.apply_settings(&config.audio, gains);
                }
                // 模式变了就重新给 WM 提示（Wayland 下是 no-op，但仍保留正确语义）。
                if let Some(window) = &self.window {
                    window.set_window_level(if config.appearance.mode == "wallpaper" {
                        WindowLevel::AlwaysOnBottom
                    } else {
                        WindowLevel::AlwaysOnTop
                    });
                    let _ = window.set_cursor_hittest(false);
                }
                eprintln!("配置已应用（revision {revision}）");
            }
        }

        // 隐藏 = 销毁窗口，显示 = 重建窗口（Wayland 没有 set_visible）。
        let visible = self.overlay_visible.load(Ordering::Relaxed);
        // 后台轮询的默认节拍（配置热重载 / IPC），有帧要画时会被帧截止时间提前。
        let mut wake = Instant::now() + Duration::from_millis(250);
        if visible {
            if self.window.is_none() {
                self.open_window(event_loop);
            }
            if let Some(state) = &self.state {
                if state.ready_for_frame() {
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                } else {
                    let deadline = state.next_frame_deadline();
                    if deadline < wake {
                        wake = deadline;
                    }
                }
            }
        } else if self.window.is_some() {
            self.hide_window();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let instance = match SingleInstance::acquire() {
        Acquire::Primary(instance) => instance,
        Acquire::AlreadyRunning => {
            eprintln!("已有一个 VisualSpectrum 在跑 —— 已请求它显示可视化");
            return Ok(());
        }
    };

    let mut store = ConfigStore::with_default_path();
    store.load_or_default(true);
    if let Some(error) = store.last_error() {
        eprintln!("配置读取有问题：{error}（已用默认值）");
    }
    let store = Arc::new(Mutex::new(store));
    let live = Arc::new(Mutex::new(vs_server::Live::default()));
    let overlay_visible = Arc::new(AtomicBool::new(true));
    let quit = Arc::new(AtomicBool::new(false));
    let tray = tray::spawn(Arc::clone(&overlay_visible), Arc::clone(&quit));
    let initial_revision = store
        .lock()
        .map(|store| store.revision())
        .unwrap_or_default();
    vs_server::spawn(
        vs_server::Shared::new(
            Arc::clone(&store),
            Arc::clone(&live),
            Arc::clone(&overlay_visible),
        ),
        vs_server::DEFAULT_PORT,
    );

    let event_loop = EventLoop::new()?;
    let mut app = App {
        window: None,
        state: None,
        store,
        live,
        overlay_visible,
        quit,
        _tray: tray,
        engine: None,
        instance,
        last_config_check: Instant::now(),
        last_applied_revision: initial_revision,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    /// 着色器不依赖 GPU 也能先验证一遍语法 / 类型 —— 这个环境跑不了窗口，但能跑它。
    fn validate(source: &str) {
        let module = naga::front::wgsl::parse_str(source).expect("WGSL 解析失败");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator.validate(&module).expect("WGSL 校验失败");
    }

    #[test]
    fn shaders_are_valid_wgsl() {
        validate(include_str!("shader.wgsl"));
        validate(include_str!("bloom.wgsl"));
        validate(include_str!("afterglow.wgsl"));
        validate(include_str!("particle.wgsl"));
        validate(include_str!("center.wgsl"));
        validate(include_str!("crt.wgsl"));
    }

    /// 中心 uniform 的布局必须和 `center.wgsl` 里手算的对得上，否则环色会整体错位。
    #[test]
    fn center_uniform_layout_matches_shader() {
        use super::CenterUniforms;
        use std::mem::{offset_of, size_of};
        assert_eq!(offset_of!(CenterUniforms, stops), 48);
        assert_eq!(offset_of!(CenterUniforms, ring_colors), 176);
        assert_eq!(size_of::<CenterUniforms>(), 336);
    }
}
