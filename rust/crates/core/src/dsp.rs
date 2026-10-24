//! 音频分析：频段划分 / 聚合 / 平滑 / demo 波形。
//!
//! 这里只有**纯函数**（`band_bins` / `map_bands` / `SpectrumProcessor` / `demo_frame`），
//! 不碰音频设备，方便单测；真正的设备抓取在 `vs-audio`。

use std::f64::consts::PI;

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

/// 采样率 / FFT 长度 —— 2048 @ 48kHz ≈ 43ms 一帧，频率分辨率 ≈ 23Hz。
pub const SAMPLE_RATE: usize = 48_000;
pub const FFT_SIZE: usize = 2_048;

/// 频段覆盖范围（Hz）。低于 30Hz 基本是直流/噪声，高于 ~16kHz 人耳听不到。
pub const F_MIN: f64 = 30.0;
pub const F_MAX: f64 = 16_000.0;

/// demo 模式每秒出多少帧。
pub const DEMO_FPS: f64 = 60.0;

/// 低 / 中 / 高音的分界（占频段数的比例）。频段按对数均分，所以 0.34 ≈ 250Hz、0.78 ≈ 4kHz。
pub const REGION_SPLIT: (f64, f64) = (0.34, 0.78);

// ==========================================================================
// 纯函数：频段划分与聚合
// ==========================================================================

/// 把 `rfft` 的输出（长 `fft_size/2 + 1`）切成 `count` 段，返回每段的 `[lo, hi)`。
///
/// 频段按**对数**均分（音乐是八度的，不是线性的）：低频窄、高频宽。
/// 每段保证至少 1 个 bin。
pub fn band_bins(
    count: usize,
    sample_rate: usize,
    fft_size: usize,
    f_min: f64,
    f_max: f64,
) -> Vec<(usize, usize)> {
    assert!(count >= 1, "count 至少要 1");
    let bins = fft_size / 2 + 1;
    let nyquist = sample_rate as f64 / 2.0;
    let mut hi_hz = f_max.min(nyquist * 0.98);
    let lo_hz = f_min.max(20.0);
    if hi_hz <= lo_hz {
        hi_hz = lo_hz * 2.0;
    }

    let ratio = hi_hz / lo_hz;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let lo_hz_i = lo_hz * ratio.powf(i as f64 / count as f64);
        let hi_hz_i = lo_hz * ratio.powf((i + 1) as f64 / count as f64);
        let lo = (lo_hz_i / nyquist * (bins - 1) as f64).round() as isize;
        let hi = (hi_hz_i / nyquist * (bins - 1) as f64).round() as isize;
        let lo = lo.clamp(0, bins as isize - 1) as usize;
        let hi = hi.max(lo as isize + 1).min(bins as isize) as usize;
        out.push((lo, hi));
    }
    out
}

/// 把每个频段里的幅度取平均。
pub fn map_bands(magnitudes: &[f64], ranges: &[(usize, usize)]) -> Vec<f64> {
    ranges
        .iter()
        .map(|&(lo, hi)| {
            let hi = hi.min(magnitudes.len());
            let lo = lo.min(hi);
            let segment = &magnitudes[lo..hi];
            if segment.is_empty() {
                0.0
            } else {
                segment.iter().sum::<f64>() / segment.len() as f64
            }
        })
        .collect()
}

/// 对一帧**已混成单路**的采样做 FFT，聚合成频段幅度。
///
/// 与 numpy 的 `rfft` 对齐：不足 `fft_size` 尾部补零，超过取最后一截，Hann 窗按 `N-1` 归一。
pub fn bands_of(samples: &[f32], fft_size: usize, ranges: &[(usize, usize)]) -> Vec<f64> {
    let mut buffer: Vec<Complex<f64>> = vec![Complex::new(0.0, 0.0); fft_size];
    let denom = fft_size.saturating_sub(1).max(1) as f64;
    let longer = samples.len() > fft_size;
    let start = samples.len().saturating_sub(fft_size);

    for (i, slot) in buffer.iter_mut().enumerate() {
        let sample = if longer {
            samples[start + i] as f64
        } else if i < samples.len() {
            samples[i] as f64
        } else {
            0.0
        };
        let window = 0.5 - 0.5 * (2.0 * PI * i as f64 / denom).cos();
        *slot = Complex::new(sample * window, 0.0);
    }

    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(fft_size);
    fft.process(&mut buffer);

    let magnitudes: Vec<f64> = buffer
        .iter()
        .take(fft_size / 2 + 1)
        .map(|c| c.norm())
        .collect();
    map_bands(&magnitudes, ranges)
}

/// 立体声（交错存放）采样 → 先平均成单路，再做 [`bands_of`]。
///
/// 平均比"只取第一路"稳（某一路可能是空的 / 相位相反）。
pub fn bands_of_interleaved(
    samples: &[f32],
    channels: usize,
    fft_size: usize,
    ranges: &[(usize, usize)],
) -> Vec<f64> {
    if channels <= 1 {
        return bands_of(samples, fft_size, ranges);
    }
    let frames = samples.len() / channels;
    let mut mono = Vec::with_capacity(frames);
    for frame in 0..frames {
        let base = frame * channels;
        let sum: f32 = samples[base..base + channels].iter().sum();
        mono.push(sum / channels as f32);
    }
    bands_of(&mono, fft_size, ranges)
}

// ==========================================================================
// 平滑 + 自动增益
// ==========================================================================

/// 把 `count` 个频段切成低/中/高三块，返回 `[(lo, hi, 区域号)]`。
pub fn region_bounds(count: usize) -> Vec<(usize, usize, usize)> {
    if count < 3 {
        return vec![(0, count, 0)];
    }
    let (lo_split, hi_split) = REGION_SPLIT;
    let b1 = ((count as f64 * lo_split).round() as usize)
        .max(1)
        .min(count - 2);
    let b2 = ((count as f64 * hi_split).round() as usize)
        .max(b1 + 1)
        .min(count - 1);
    vec![(0, b1, 0), (b1, b2, 1), (b2, count, 2)]
}

/// 把原始幅度变成"好看、稳定"的 0~1 电平。
///
/// 四步（顺序有意）：分区域自动增益 → 占比增益 → 伽马 → 攻击/释放平滑。
/// 分区域是关键：低音不会再永远打满（以前用全局峰值做参考线，低音通常最响，于是永远顶格）。
#[derive(Debug, Clone)]
pub struct SpectrumProcessor {
    pub count: usize,
    pub attack: f64,
    pub release: f64,
    pub gamma: f64,
    /// 参考线回落速度（每帧向当前峰值靠拢多少）—— 越小越"长记忆"，起伏越明显。
    pub ref_falloff: f64,
    pub gains: [f64; 3],
    regions: Vec<(usize, usize, usize)>,
    global_ref: f64,
    refs: [f64; 3],
    values: Vec<f64>,
}

impl SpectrumProcessor {
    /// 默认参数（与配置默认值一致）。
    pub fn new(count: usize, gains: [f64; 3]) -> Self {
        Self::with_params(count, 0.75, 0.30, 0.75, 0.05, gains)
    }

    pub fn with_params(
        count: usize,
        attack: f64,
        release: f64,
        gamma: f64,
        ref_falloff: f64,
        gains: [f64; 3],
    ) -> Self {
        Self {
            count,
            attack,
            release,
            gamma,
            ref_falloff,
            gains,
            regions: region_bounds(count),
            global_ref: 1e-6,
            refs: [1e-6; 3],
            values: vec![0.0; count],
        }
    }

    /// 更新低 / 中 / 高的占比倍数。
    pub fn set_gains(&mut self, gains: [f64; 3]) {
        self.gains = gains;
    }

    pub fn reset(&mut self, count: Option<usize>) {
        if let Some(count) = count {
            self.count = count;
            self.regions = region_bounds(count);
        }
        self.global_ref = 1e-6;
        self.refs = [1e-6; 3];
        self.values = vec![0.0; self.count];
    }

    /// `bands` = 原始线性幅度；返回等长的 0~1 电平（就地更新内部状态）。
    pub fn process(&mut self, bands: &[f64]) -> Vec<f64> {
        if bands.len() != self.count {
            self.reset(Some(bands.len()));
        }

        let falloff = self.ref_falloff;
        let peak_all = bands.iter().copied().fold(0.0_f64, f64::max);
        self.global_ref = if peak_all > self.global_ref {
            peak_all
        } else {
            self.global_ref + (peak_all - self.global_ref) * falloff
        };
        // 整块区域都安静时的"参考线地板"—— 免得分母趋近 0 把噪声放大成满格。
        let floor = (self.global_ref * 0.05).max(1e-6);

        let mut out = vec![0.0; self.count];
        for &(lo, hi, region) in &self.regions {
            let hi = hi.min(bands.len());
            let lo = lo.min(hi);
            let peak = bands[lo..hi].iter().copied().fold(0.0_f64, f64::max);
            let mut reference = self.refs[region];
            reference = if peak > reference {
                peak
            } else {
                reference + (peak - reference) * falloff
            };
            reference = reference.max(floor);
            self.refs[region] = reference;

            let gain = self.gains[region] / reference;
            for i in lo..hi {
                let x = (bands[i] * gain).clamp(0.0, 1.0).powf(self.gamma);
                let prev = self.values[i];
                let k = if x > prev { self.attack } else { self.release };
                let y = prev + (x - prev) * k;
                self.values[i] = y;
                out[i] = y;
            }
        }
        out
    }
}

// ==========================================================================
// demo 源
// ==========================================================================

/// 一个很小的确定性随机源（xorshift64*）—— demo 波形用，不引入 rand 依赖。
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// `[0, 1)` 的浮点。
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// demo 里一团在频轴上漂移的能量：`(起始相位, 漂移速度, 宽度)`。
#[derive(Debug, Clone, Copy)]
pub struct DemoBlob {
    pub phase: f64,
    pub speed: f64,
    pub width: f64,
}

/// 合成一帧"像音乐"的幅度谱：低频跟着节拍鼓动 + 几团漂移能量 + 一点底噪。
pub fn demo_frame(t: f64, count: usize, blobs: &[DemoBlob], rng: &mut Rng) -> Vec<f64> {
    let denom = count.saturating_sub(1).max(1) as f64;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let x = i as f64 / denom;
        let mut value = 0.05 + rng.next_f64() * 0.05;
        // 底鼓：低频（x≈0）上一记 2Hz 的脉冲。
        let kick = ((x - 0.02) / 0.07).powi(2);
        value += 0.9 * (-kick).exp() * (0.5 + 0.5 * (2.0 * PI * 2.0 * t).sin());
        // 漂移的能量团。
        for blob in blobs {
            let center = (blob.phase + blob.speed * t) % 1.0;
            let delta = x - center;
            value += 0.55 * (-(delta / blob.width).powi(2)).exp();
        }
        out.push(value);
    }
    out
}
