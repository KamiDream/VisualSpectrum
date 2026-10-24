//! 音频引擎：抓系统声音（回环 / monitor）→ FFT → 频段电平。
//!
//! 两条抓取路径：
//! 1. **pulse / PipeWire（首选）**：`parec` 录默认输出的 `<sink>.monitor`，裸 f32le 交给 FFT；
//!    `auto` 会每 2 秒检查默认 sink，切蓝牙 / HDMI 时自动重连。
//! 2. **cpal**：显式配置 `cpal:<设备名>` 或 pulse 不可用时使用
//!    （注意：PipeWire 下 cpal 的默认源通常是**麦克风**）。
//!
//! 两条都失败时退到 **demo**（合成频谱，画面永远是活的）。
//! 纯分析逻辑在 [`vs_core::dsp`]，这里只负责把设备数据喂进去。

mod devices;
mod engine;

pub use devices::{list_devices, pick_device, DeviceInfo};
pub use engine::{AudioEngine, EngineError, Snapshot};
