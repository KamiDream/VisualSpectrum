//! 引擎：在后台线程里抓音频并维护"最新一帧"。

use std::fmt;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::Sample;

use vs_core::dsp::{
    band_bins, bands_of, demo_frame, DemoBlob, Rng, SpectrumProcessor, DEMO_FPS, FFT_SIZE, F_MAX,
    F_MIN,
};

use crate::devices::{default_monitor_source, pick_device};

/// 引擎错误（消息直接给日志 / 用户看）。
#[derive(Debug, Clone)]
pub enum EngineError {
    NoDevice(String),
    Device(String),
    Build(String),
    Play(String),
    Unsupported(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::NoDevice(msg)
            | EngineError::Device(msg)
            | EngineError::Build(msg)
            | EngineError::Play(msg)
            | EngineError::Unsupported(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for EngineError {}

/// 当前这一帧：`bands` 是 `count` 个 0~1 电平。
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub bands: Vec<f64>,
    pub live: bool,
    pub source: String,
}

impl Snapshot {
    fn idle(count: usize) -> Self {
        Self {
            bands: vec![0.0; count],
            live: false,
            source: "demo（未启动）".to_string(),
        }
    }
}

/// 引擎的不可变运行参数（改了就重启工作线程）。
#[derive(Debug, Clone)]
struct EngineConfig {
    count: usize,
    fft_size: usize,
    f_min: f64,
    f_max: f64,
    source: String,
    delay_ms: i64,
    device: String,
    gains: [f64; 3],
}

impl EngineConfig {
    fn new(count: usize, audio: &vs_core::Audio, gains: [f64; 3]) -> Self {
        Self {
            count,
            fft_size: FFT_SIZE,
            f_min: F_MIN,
            f_max: F_MAX,
            source: if audio.source == "off" {
                "off".into()
            } else {
                "system".into()
            },
            delay_ms: audio.delay_ms.max(0),
            device: audio.device.trim().to_string(),
            gains,
        }
    }
}

/// 音频引擎：`start()` 起后台线程，`snapshot()` 随时取最新一帧。
pub struct AudioEngine {
    shared: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    config: EngineConfig,
}

impl AudioEngine {
    pub fn new(count: usize, audio: &vs_core::Audio, gains: [f64; 3]) -> Self {
        Self {
            shared: Arc::new(Mutex::new(Snapshot::idle(count))),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
            config: EngineConfig::new(count, audio, gains),
        }
    }

    pub fn start(&mut self) {
        self.stop_worker();
        self.stop.store(false, Ordering::SeqCst);
        let config = self.config.clone();
        let shared = Arc::clone(&self.shared);
        let stop = Arc::clone(&self.stop);
        self.worker = Some(thread::spawn(move || worker(config, shared, stop)));
    }

    pub fn stop(&mut self) {
        self.stop_worker();
    }

    fn stop_worker(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }

    /// 来源 / 设备改了要重开捕获；只改占比则不必重启。
    pub fn apply_settings(&mut self, audio: &vs_core::Audio, gains: [f64; 3]) {
        let restart = audio.source != self.config.source || audio.device != self.config.device;
        self.config = EngineConfig::new(self.config.count, audio, gains);
        if restart && self.worker.is_some() {
            self.start();
        }
    }

    /// 数量改了 —— 重开（频段划分和处理器状态都要换）。
    pub fn reconfigure(&mut self, count: usize, gains: [f64; 3]) {
        let restart = count != self.config.count || gains != self.config.gains;
        self.config.count = count;
        self.config.gains = gains;
        if restart && self.worker.is_some() {
            self.start();
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| Snapshot::idle(self.config.count))
    }

    pub fn running(&self) -> bool {
        self.worker.is_some()
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

// ==========================================================================
// 后台线程
// ==========================================================================

fn worker(config: EngineConfig, shared: Arc<Mutex<Snapshot>>, stop: Arc<AtomicBool>) {
    if config.source == "off" {
        set_source(&shared, "demo（已关闭捕获）");
        run_demo(&config, &shared, &stop);
        return;
    }

    if config.delay_ms > 0 {
        let deadline = Instant::now() + Duration::from_millis(config.delay_ms as u64);
        while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
    }
    if stop.load(Ordering::Relaxed) {
        return;
    }

    if let Err(err) = run_real(&config, &shared, &stop) {
        eprintln!("回环音频打不开（{err}）—— 改用 demo 模式");
        if !stop.load(Ordering::Relaxed) {
            set_source(&shared, "demo（合成）");
            run_demo(&config, &shared, &stop);
        }
    }
}

fn set_source(shared: &Arc<Mutex<Snapshot>>, label: &str) {
    if let Ok(mut snapshot) = shared.lock() {
        snapshot.live = false;
        snapshot.source = label.to_string();
    }
}

fn run_real(
    config: &EngineConfig,
    shared: &Arc<Mutex<Snapshot>>,
    stop: &Arc<AtomicBool>,
) -> Result<(), EngineError> {
    let wanted = config.device.trim();
    // `pulse:<源名>` 明确指定回环；`auto` / 空也优先用"默认输出的 monitor"。
    // 这是修"音乐在放但频谱不动"的关键：cpal 的 ALSA 设备默认连的是**麦克风**。
    if let Some(name) = wanted.strip_prefix("pulse:") {
        let name = name.trim();
        if !name.is_empty() && name != "auto" {
            match run_pulse_session(config, shared, stop, name, false) {
                PulseEnd::Stopped | PulseEnd::SourceChanged => return Ok(()),
                PulseEnd::Failed(err) => {
                    eprintln!("pulse 设备 {name} 捕获失败（{err}）—— 改跟默认输出");
                }
            }
        }
        if default_monitor_source().is_some() {
            match run_pulse_auto(config, shared, stop) {
                Ok(()) => return Ok(()),
                Err(_) if stop.load(Ordering::Relaxed) => return Ok(()),
                Err(err) => eprintln!("pulse 捕获失败（{err}）—— 退回 cpal"),
            }
        }
        return run_cpal(config, shared, stop);
    }
    if !wanted.starts_with("cpal:") && default_monitor_source().is_some() {
        match run_pulse_auto(config, shared, stop) {
            Ok(()) => return Ok(()),
            Err(_) if stop.load(Ordering::Relaxed) => return Ok(()),
            Err(err) => eprintln!("pulse 捕获失败（{err}）—— 退回 cpal"),
        }
    }
    run_cpal(config, shared, stop)
}

/// 一次 parec 会话的结束原因。
enum PulseEnd {
    /// 被主动停止（重启 / 退出）。
    Stopped,
    /// 默认输出换了（蓝牙/HDMI 切换）—— 外层重新解析 monitor。
    SourceChanged,
    /// parec 退出 / 读失败。
    Failed(String),
}

/// 跟着"默认输出"走：设备换了就重连，parec 意外退出也重试。
///
/// 这是修"切到蓝牙耳机后频谱不动"的那一层：`auto` 不能只在启动时解析一次。
fn run_pulse_auto(
    config: &EngineConfig,
    shared: &Arc<Mutex<Snapshot>>,
    stop: &Arc<AtomicBool>,
) -> Result<(), EngineError> {
    let mut last_error: Option<String> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let Some(source) = default_monitor_source() else {
            return Err(EngineError::Device(last_error.unwrap_or_else(|| {
                "找不到默认输出的回环源（parec / pactl 不可用）".to_string()
            })));
        };
        match run_pulse_session(config, shared, stop, &source, true) {
            PulseEnd::Stopped => return Ok(()),
            PulseEnd::SourceChanged => {
                if let Ok(mut snapshot) = shared.lock() {
                    snapshot.live = false;
                }
                eprintln!("默认输出变了 —— 重新连接回环");
            }
            PulseEnd::Failed(err) => {
                eprintln!("pulse 捕获中断（{err}）—— 1s 后重试");
                last_error = Some(err);
                let deadline = Instant::now() + Duration::from_secs(1);
                while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
}

/// 用 `parec` 录**指定源**（通常是 `<默认输出>.monitor` = 系统正在播放的声音）。
///
/// 为什么不直接用 cpal：PipeWire 下 cpal 走 ALSA 插件，默认源是麦克风；而系统回环
/// 只有 PulseAudio/PipeWire 的 monitor 源暴露得最全。`parec` 输出裸 f32le，处理起来直接。
fn run_pulse_session(
    config: &EngineConfig,
    shared: &Arc<Mutex<Snapshot>>,
    stop: &Arc<AtomicBool>,
    source: &str,
    follow_default: bool,
) -> PulseEnd {
    const RATE: usize = 44_100;
    const CHANNELS: usize = 2;
    let mut child = match Command::new("parec")
        .arg(format!("--device={source}"))
        .args([
            "--format=float32le",
            "--rate=44100",
            "--channels=2",
            "--latency-msec=20",
            "--raw",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => return PulseEnd::Failed(format!("启动 parec 失败：{err}")),
    };

    let mut capture = Capture {
        fft_size: config.fft_size,
        hop: (config.fft_size / 4).max(1),
        ranges: band_bins(
            config.count,
            RATE,
            config.fft_size,
            config.f_min,
            config.f_max,
        ),
        processor: SpectrumProcessor::new(config.count, config.gains),
        mono: Vec::with_capacity(config.fft_size),
        shared: Arc::clone(shared),
        source_name: format!("pulse · {source}（{RATE}Hz/{CHANNELS}ch）"),
    };

    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        return PulseEnd::Failed("拿不到 parec 的输出".to_string());
    };
    let frame_bytes = 4 * CHANNELS;
    let mut buffer = vec![0_u8; 8192];
    if let Ok(mut snapshot) = shared.lock() {
        snapshot.source = capture.source_name.clone();
        snapshot.live = false;
    }
    let mut end = PulseEnd::Stopped;
    let mut last_check = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match stdout.read(&mut buffer) {
            Ok(0) => {
                end = PulseEnd::Failed("parec 已退出（设备可能被移除）".to_string());
                break;
            }
            Ok(n) => {
                // 只处理完整的帧（read 可能切在采样中间）。
                let usable = n - (n % frame_bytes);
                for frame in buffer[..usable].chunks_exact(frame_bytes) {
                    let mut sum = 0.0_f32;
                    for channel in 0..CHANNELS {
                        let offset = channel * 4;
                        sum += f32::from_le_bytes([
                            frame[offset],
                            frame[offset + 1],
                            frame[offset + 2],
                            frame[offset + 3],
                        ]);
                    }
                    capture.push(sum / CHANNELS as f32);
                }
                // 每 2s 看一眼默认输出有没有换（蓝牙 / HDMI / 拔插耳机）。
                if follow_default && last_check.elapsed() >= Duration::from_secs(2) {
                    last_check = Instant::now();
                    if let Some(next) = default_monitor_source() {
                        if next != source {
                            end = PulseEnd::SourceChanged;
                            break;
                        }
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                end = PulseEnd::Failed(format!("读 parec 失败：{err}"));
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    if stop.load(Ordering::Relaxed) {
        PulseEnd::Stopped
    } else {
        end
    }
}

fn run_cpal(
    config: &EngineConfig,
    shared: &Arc<Mutex<Snapshot>>,
    stop: &Arc<AtomicBool>,
) -> Result<(), EngineError> {
    let device = pick_device(&config.device)?;
    let supported = device
        .default_input_config()
        .map_err(|err| EngineError::Device(err.to_string()))?;
    let sample_rate = supported.sample_rate().0 as usize;
    let channels = supported.channels() as usize;
    let sample_format = supported.sample_format();
    let stream_config = supported.config();
    let name = device.name().unwrap_or_else(|_| "音频输入".to_string());
    let source_name = format!("cpal · {name}（{sample_rate}Hz/{channels}ch）");

    let capture = Capture {
        fft_size: config.fft_size,
        hop: (config.fft_size / 4).max(1),
        ranges: band_bins(
            config.count,
            sample_rate,
            config.fft_size,
            config.f_min,
            config.f_max,
        ),
        processor: SpectrumProcessor::new(config.count, config.gains),
        mono: Vec::with_capacity(config.fft_size),
        shared: Arc::clone(shared),
        source_name: source_name.clone(),
    };
    let error_shared = Arc::clone(shared);
    let on_error = move |err: cpal::StreamError| {
        eprintln!("音频流错误：{err}");
        if let Ok(mut snapshot) = error_shared.lock() {
            snapshot.live = false;
        }
    };

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            build_stream::<f32>(&device, &stream_config, channels, capture, on_error)?
        }
        cpal::SampleFormat::I16 => {
            build_stream::<i16>(&device, &stream_config, channels, capture, on_error)?
        }
        cpal::SampleFormat::U16 => {
            build_stream::<u16>(&device, &stream_config, channels, capture, on_error)?
        }
        other => {
            return Err(EngineError::Unsupported(format!(
                "不支持的采样格式 {other:?}"
            )))
        }
    };
    stream
        .play()
        .map_err(|err| EngineError::Play(err.to_string()))?;

    if let Ok(mut snapshot) = shared.lock() {
        snapshot.source = source_name;
        snapshot.live = true;
    }

    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(50));
    }
    drop(stream);
    Ok(())
}

/// 把交错采样平均成单路，攒够 `fft_size` 就分析一帧。
struct Capture {
    fft_size: usize,
    /// 每次分析后丢弃的旧样本数（hop = 1/4 窗 = 75% 重叠 → 约 86 次/秒，比不重叠的 21 次顺很多）。
    hop: usize,
    ranges: Vec<(usize, usize)>,
    processor: SpectrumProcessor,
    mono: Vec<f32>,
    shared: Arc<Mutex<Snapshot>>,
    source_name: String,
}

impl Capture {
    fn push(&mut self, value: f32) {
        self.mono.push(value);
        while self.mono.len() >= self.fft_size {
            let bands = bands_of(&self.mono[..self.fft_size], self.fft_size, &self.ranges);
            let levels = self.processor.process(&bands);
            if let Ok(mut snapshot) = self.shared.lock() {
                snapshot.bands = levels;
                snapshot.live = true;
                snapshot.source = self.source_name.clone();
            }
            // 留一半样本给下一帧重叠，而不是全部清空。
            self.mono.drain(..self.hop.min(self.mono.len()));
        }
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    mut capture: Capture,
    on_error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, EngineError>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _info: &cpal::InputCallbackInfo| {
                for frame in data.chunks(channels) {
                    let mut sum = 0.0f32;
                    for &sample in frame {
                        sum += f32::from_sample(sample);
                    }
                    capture.push(sum / channels as f32);
                }
            },
            on_error,
            None,
        )
        .map_err(|err| EngineError::Build(err.to_string()))
}

fn default_blobs() -> Vec<DemoBlob> {
    let mut rng = Rng::new(20_261_010);
    (0..5)
        .map(|_| DemoBlob {
            phase: rng.next_f64(),
            speed: 0.03 + rng.next_f64() * 0.05,
            width: 0.05 + rng.next_f64() * 0.05,
        })
        .collect()
}

fn run_demo(config: &EngineConfig, shared: &Arc<Mutex<Snapshot>>, stop: &Arc<AtomicBool>) {
    let blobs = default_blobs();
    let mut rng = Rng::new(0x5EED_1234);
    let mut processor = SpectrumProcessor::new(config.count, config.gains);
    let mut t = 0.0;
    let interval = Duration::from_secs_f64(1.0 / DEMO_FPS);
    while !stop.load(Ordering::Relaxed) {
        let raw = demo_frame(t, config.count, &blobs, &mut rng);
        let levels = processor.process(&raw);
        if let Ok(mut snapshot) = shared.lock() {
            snapshot.bands = levels;
            snapshot.live = false;
        }
        t += 1.0 / DEMO_FPS;
        thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_snapshot_has_the_right_length() {
        let audio = vs_core::Audio::default();
        let engine = AudioEngine::new(32, &audio, [1.0, 1.0, 1.0]);
        let snapshot = engine.snapshot();
        assert_eq!(snapshot.bands.len(), 32);
        assert!(!snapshot.live);
        assert!(!engine.running());
    }

    #[test]
    fn demo_source_does_not_touch_audio_devices() {
        let audio = vs_core::Audio {
            source: "off".to_string(),
            ..Default::default()
        };
        let mut engine = AudioEngine::new(24, &audio, [1.0, 1.0, 1.0]);
        engine.start();
        std::thread::sleep(Duration::from_millis(200));
        let snapshot = engine.snapshot();
        engine.stop();
        assert_eq!(snapshot.bands.len(), 24);
        assert!(!snapshot.live);
        assert!(snapshot.source.contains("demo"));
        assert!(snapshot.bands.iter().all(|v| v.is_finite()));
    }
}
