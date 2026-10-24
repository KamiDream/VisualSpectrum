//! VisualSpectrum 的**纯逻辑核心**（Rust 重写的地基）。
//!
//! 这一层刻意不碰窗口、GPU、音频设备 —— 只做两件事：
//!
//! * [`dsp`]：把一帧采样变成 `count` 个 0~1 的频段电平；
//! * [`config`]：配置 schema / 严格校验 / 原子读写 / 热重载。
//!
//! 为什么先做这一层：它跟"用 wgpu 画还是用别的画"完全无关，而且可以用单元测试
//! 逐个钉住行为 —— 先把地基验证对，再往上接音频设备、HTTP、窗口和渲染。

pub mod config;
pub mod dsp;

pub use config::{
    appearance_of, audio_of, default_config_path, render_of, validate, Appearance, Audio, Config,
    ConfigError, ConfigStore, ReloadResult, Render, AFTERGLOW_RANGE, AUDIO_SOURCES,
    BG_OPACITY_RANGE, CENTER_GLOW_RANGE, COLORS_MAX, COUNT_RANGE, CRT_FX_RANGE, DEFAULT_COLORS,
    DELAY_RANGE, HEIGHT_RANGE, INTENSITY_RANGE, MIX_RANGE, MODES, PARTICLE_RANGE,
    ROTATE_SPEED_RANGE, SENSITIVITY_RANGE, SIZE_RANGE, VERSION,
};
pub use dsp::{
    band_bins, bands_of, bands_of_interleaved, demo_frame, map_bands, region_bounds, DemoBlob, Rng,
    SpectrumProcessor, DEMO_FPS, FFT_SIZE, F_MAX, F_MIN, REGION_SPLIT, SAMPLE_RATE,
};
