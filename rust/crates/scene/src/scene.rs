//! 场景状态机：每帧推进电平 / 自转 / 粒子 / 鼓点，并产出可绘制几何。

use std::f64::consts::{PI, TAU};

use vs_core::{Appearance, Rng};

/// 同时存在的粒子上限（和 JS 版一致）。
const MAX_PARTICLES: usize = 700;
/// 每帧最多尝试生成的粒子数上限。
const MAX_SPAWN_PER_TICK: usize = 24;
/// 一次鼓点爆发最多多少粒子。
const MAX_BURST: usize = 140;
/// 显示电平的跟随速度（指数平滑，帧率无关，数值 = 1/时间常数）。
/// 之前是 90.86 / 23.14（上升 τ≈11ms），实际效果就是"瞬跳"；
/// 现在上升 τ≈45ms、下降 τ≈154ms，能看出起落动画但不会拖遢。
const ATTACK_RATE: f64 = 22.0;
const RELEASE_RATE: f64 = 6.5;

/// 视口尺寸（逻辑像素）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub width: f64,
    pub height: f64,
}

/// 环的几何量（都由视口 + 配置算出来）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub cx: f64,
    pub cy: f64,
    pub radius_outer: f64,
    pub radius_base: f64,
    pub max_len: f64,
}

/// 一根频谱条（渲染器拿来画线/胶囊）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub width: f64,
    pub value: f64,
    pub color: [f64; 3],
}

/// 一颗散射粒子。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    pub angle: f64,
    pub radius: f64,
    pub speed: f64,
    pub life: f64,
    pub ttl: f64,
    pub size: f64,
    pub color: [f64; 3],
}

impl Particle {
    /// 0 → 刚生成，1 → 正好消失。
    pub fn progress(&self) -> f64 {
        if self.ttl <= 0.0 {
            1.0
        } else {
            (self.life / self.ttl).clamp(0.0, 1.0)
        }
    }

    /// 渐隐系数（1 → 0）。
    pub fn fade(&self) -> f64 {
        1.0 - self.progress()
    }

    pub fn position(&self, viewport: Viewport) -> (f64, f64) {
        let cx = viewport.width / 2.0;
        let cy = viewport.height / 2.0;
        (
            cx + self.angle.cos() * self.radius,
            cy + self.angle.sin() * self.radius,
        )
    }
}

/// 环的几何：外径由 `size` 决定，条从内往外长；`height > 100%` 时底座收到圆心（不封顶）。
pub fn geometry(viewport: Viewport, appearance: &Appearance) -> Geometry {
    let cx = viewport.width / 2.0;
    let cy = viewport.height / 2.0;
    let radius_outer =
        (appearance.size.clamp(5, 200) as f64 / 100.0) * viewport.width.min(viewport.height) / 2.0;
    let h = (appearance.height as f64 / 100.0).clamp(0.05, 5.0);
    let max_len = radius_outer * h;
    let radius_base = (radius_outer * (1.0 - h)).max(0.0);
    Geometry {
        cx,
        cy,
        radius_outer,
        radius_base,
        max_len,
    }
}

/// `#RRGGBB` → `[r, g, b]`（解析不了退成青色，和 JS 版一致）。
pub fn parse_hex(hex: &str) -> [f64; 3] {
    let text = hex.trim();
    let bytes = text.as_bytes();
    if bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit) {
        let value = u32::from_str_radix(&text[1..], 16).unwrap_or(0);
        return [
            f64::from((value >> 16) & 0xFF),
            f64::from((value >> 8) & 0xFF),
            f64::from(value & 0xFF),
        ];
    }
    [126.0, 231.0, 255.0]
}

/// 把色标均匀铺在 `n` 份上并插值：1 色 = 纯色，多色 = 渐变。
pub fn palette(appearance: &Appearance, n: usize) -> Vec<[f64; 3]> {
    let stops: Vec<[f64; 3]> = if appearance.colors.is_empty() {
        vec![parse_hex("#7EE7FF")]
    } else {
        appearance.colors.iter().map(|c| parse_hex(c)).collect()
    };
    if n == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let pos = if n > 1 {
            i as f64 / (n - 1) as f64
        } else {
            0.0
        };
        let scaled = pos * (stops.len() - 1) as f64;
        let lo = scaled.floor() as usize;
        let hi = (lo + 1).min(stops.len() - 1);
        let t = scaled - lo as f64;
        let a = stops[lo];
        let b = stops[hi];
        out.push([
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]);
    }
    out
}

/// 余晖（荧光拖尾）强度 → 每秒衰减速率。0 = 每帧全清（返回无穷大）。
pub fn afterglow_rate(level: f64) -> f64 {
    let v = level.clamp(0.0, 100.0);
    if v <= 0.0 {
        return f64::INFINITY;
    }
    lerp(24.0, 2.0, (v - 1.0) / 99.0)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn resample(bands: &[f64], count: usize) -> Vec<f64> {
    if count == 0 {
        return Vec::new();
    }
    if bands.is_empty() {
        return vec![0.0; count];
    }
    if bands.len() == 1 {
        return vec![bands[0]; count];
    }
    let n = bands.len();
    (0..count)
        .map(|i| {
            let pos = i as f64 / (count - 1) as f64 * (n - 1) as f64;
            let lo = pos.floor() as usize;
            let hi = (lo + 1).min(n - 1);
            let t = pos - lo as f64;
            bands[lo] + (bands[hi] - bands[lo]) * t
        })
        .collect()
}

/// 场景状态机：吃频段 + 配置，吐"这一帧要画的条和粒子"。
pub struct Scene {
    target: Vec<f64>,
    shown: Vec<f64>,
    rotation: f64,
    particles: Vec<Particle>,
    bars: Vec<Bar>,
    /// 顶端细线：相邻两条的顶端中心依次相连（`appearance.link` 关闭时为空）。
    links: Vec<Bar>,
    geometry: Geometry,
    bass_avg: Option<f64>,
    since_beat: f64,
    beat_count: u64,
    rng: Rng,
}

impl Scene {
    pub fn new(seed: u64) -> Self {
        Self {
            target: Vec::new(),
            shown: Vec::new(),
            rotation: 0.0,
            particles: Vec::new(),
            bars: Vec::new(),
            links: Vec::new(),
            geometry: Geometry {
                cx: 0.0,
                cy: 0.0,
                radius_outer: 0.0,
                radius_base: 0.0,
                max_len: 0.0,
            },
            bass_avg: None,
            since_beat: 0.0,
            beat_count: 0,
            rng: Rng::new(seed),
        }
    }

    pub fn rotation(&self) -> f64 {
        self.rotation
    }

    pub fn shown(&self) -> &[f64] {
        &self.shown
    }

    pub fn bars(&self) -> &[Bar] {
        &self.bars
    }

    pub fn links(&self) -> &[Bar] {
        &self.links
    }

    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// 到目前为止检测到多少次"鼓点"（测试与调试用）。
    pub fn beat_count(&self) -> u64 {
        self.beat_count
    }

    /// 推进一帧（`dt` 秒）。`bands` 应当已经是 `count` 个 0~1 的电平（多退少补会自动重采样）。
    pub fn update(&mut self, dt: f64, viewport: Viewport, bands: &[f64], appearance: &Appearance) {
        let dt = dt.clamp(0.001, 0.05);
        let count = appearance.count.clamp(4, 512) as usize;

        self.target = if bands.len() == count {
            bands.to_vec()
        } else {
            resample(bands, count)
        };

        let gain = (appearance.intensity as f64 / 100.0).clamp(0.05, 12.0);
        if self.shown.len() != count {
            self.shown = vec![0.0; count];
        }
        let attack_k = 1.0 - (-ATTACK_RATE * dt).exp();
        let release_k = 1.0 - (-RELEASE_RATE * dt).exp();
        for (target, slot) in self.target.iter().zip(self.shown.iter_mut()) {
            let goal = (*target * gain).clamp(0.0, 1.0);
            let prev = *slot;
            let k = if goal > prev { attack_k } else { release_k };
            *slot = prev + (goal - prev) * k;
        }

        if appearance.rotate {
            // 基准 0.4 rad/s（≈16 秒一圈），配置是百分比（10~300）。
            let speed = 0.4 * (appearance.rotate_speed as f64 / 100.0).clamp(0.1, 3.0);
            self.rotation += dt * speed;
        }

        self.geometry = geometry(viewport, appearance);
        let colors = palette(appearance, count);

        self.detect_beat(dt, viewport, appearance, &colors);
        self.emit(dt, appearance, &colors);

        self.bars = self.build_bars(&colors);
        self.links = self.build_links(&colors, appearance.link);
    }

    /// 顶端连线：每条从自己的尖端连到下一条的尖端（闭合一圈）。
    ///
    /// 颜色用调色板正中间那个色再提亮 15% —— 和 JS 版 `mixWhite(mid, 0.15)` 一致；
    /// 线宽固定 1.2px（不受条宽影响，否则密集的时候会糊成一片）。
    fn build_links(&self, colors: &[[f64; 3]], enabled: bool) -> Vec<Bar> {
        let count = self.bars.len();
        if !enabled || count <= 2 {
            return Vec::new();
        }
        let mid = colors
            .get(count / 2)
            .copied()
            .unwrap_or([126.0, 231.0, 255.0]);
        let color = [
            mid[0] + (255.0 - mid[0]) * 0.15,
            mid[1] + (255.0 - mid[1]) * 0.15,
            mid[2] + (255.0 - mid[2]) * 0.15,
        ];
        let mut links = Vec::with_capacity(count);
        for (a, b) in self
            .bars
            .iter()
            .zip(self.bars.iter().cycle().skip(1))
            .take(count)
        {
            links.push(Bar {
                x0: a.x1,
                y0: a.y1,
                x1: b.x1,
                y1: b.y1,
                width: 1.2,
                value: 0.0,
                color,
            });
        }
        links
    }

    fn build_bars(&self, colors: &[[f64; 3]]) -> Vec<Bar> {
        let count = self.shown.len();
        if count == 0 {
            return Vec::new();
        }
        let g = self.geometry;
        let sector = TAU / count as f64;
        let width = (g.radius_base * sector * 0.42).max(0.9);
        let mut bars = Vec::with_capacity(count);
        for (i, (value, color)) in self.shown.iter().zip(colors.iter()).enumerate() {
            let value = *value;
            let angle = i as f64 / count as f64 * TAU - PI / 2.0 + self.rotation;
            let (sin, cos) = angle.sin_cos();
            let len = g.max_len * (0.06 + 0.94 * value);
            bars.push(Bar {
                x0: g.cx + cos * g.radius_base,
                y0: g.cy + sin * g.radius_base,
                x1: g.cx + cos * (g.radius_base + len),
                y1: g.cy + sin * (g.radius_base + len),
                width,
                value,
                color: *color,
            });
        }
        bars
    }

    fn detect_beat(
        &mut self,
        dt: f64,
        viewport: Viewport,
        appearance: &Appearance,
        colors: &[[f64; 3]],
    ) {
        if !appearance.burst {
            return;
        }
        let n = self.shown.len();
        if n == 0 {
            return;
        }
        let low = ((n as f64 * 0.08).round() as usize).max(1).min(n);
        let bass = self.shown[..low].iter().sum::<f64>() / low as f64;
        let avg = *self.bass_avg.get_or_insert(bass);

        let sens = (appearance.burst_sensitivity as f64).clamp(0.0, 100.0) / 100.0;
        let threshold = lerp(1.55, 1.04, sens);
        let refractory = lerp(0.36, 0.07, sens);
        self.since_beat += dt;

        if bass > avg * threshold && bass > 0.15 && self.since_beat > refractory {
            self.since_beat = 0.0;
            self.beat_count += 1;
            let total = (lerp(28.0, 96.0, sens)).round() as usize;
            self.spawn_burst(viewport, appearance, colors, total);
        }
        self.bass_avg = Some(avg + (bass - avg) * (dt * 1.5).min(1.0));
    }

    fn spawn_burst(
        &mut self,
        viewport: Viewport,
        appearance: &Appearance,
        colors: &[[f64; 3]],
        total: usize,
    ) {
        let n = self.shown.len().max(1);
        let life_scale = (appearance.burst_life as f64 / 100.0).clamp(0.2, 4.0);
        let speed_scale = (appearance.burst_speed as f64 / 100.0).clamp(0.2, 4.0);
        let max_radius = viewport.width.hypot(viewport.height) / 2.0;
        for _ in 0..total.clamp(0, MAX_BURST) {
            if self.particles.len() >= MAX_PARTICLES {
                break;
            }
            let ttl = (0.6 + self.rng.next_f64() * 0.9) * life_scale;
            let index = (self.rng.next_f64() * n as f64) as usize % n;
            self.particles.push(Particle {
                angle: self.rng.next_f64() * TAU,
                radius: self.rng.next_f64() * 8.0,
                speed: (max_radius / ttl) * 1.15 * speed_scale,
                life: 0.0,
                ttl,
                size: 1.4 + self.rng.next_f64() * 2.4,
                color: colors.get(index).copied().unwrap_or([126.0, 231.0, 255.0]),
            });
        }
    }

    fn emit(&mut self, dt: f64, appearance: &Appearance, colors: &[[f64; 3]]) {
        let count = self.shown.len();
        if count == 0 {
            return;
        }
        let g = self.geometry;
        let speed_scale = (appearance.particle_speed as f64 / 100.0).clamp(0.2, 4.0);
        let life_scale = (appearance.particle_life as f64 / 100.0).clamp(0.2, 4.0);
        let spawn = MAX_SPAWN_PER_TICK.min(4 + (count as f64 * 0.06).round() as usize);

        for _ in 0..spawn {
            let index = (self.rng.next_f64() * count as f64) as usize % count;
            let value = self.shown[index];
            if self.rng.next_f64() > value * value * 0.9 + 0.02 {
                continue;
            }
            if self.particles.len() >= MAX_PARTICLES {
                break;
            }
            let angle = index as f64 / count as f64 * TAU - PI / 2.0 + self.rotation;
            let start_radius = g.radius_base + g.max_len * value;
            let speed = (14.0 + value * 70.0 + self.rng.next_f64() * 20.0) * speed_scale;
            self.particles.push(Particle {
                angle: angle + (self.rng.next_f64() - 0.5) * 0.05,
                radius: start_radius,
                speed,
                life: 0.0,
                ttl: (0.7 + self.rng.next_f64() * 1.1) * life_scale,
                size: 0.8 + self.rng.next_f64() * 1.8,
                color: colors.get(index).copied().unwrap_or([126.0, 231.0, 255.0]),
            });
        }

        let mut alive = Vec::with_capacity(self.particles.len());
        for mut particle in self.particles.drain(..) {
            particle.life += dt;
            if particle.life >= particle.ttl {
                continue;
            }
            particle.radius += particle.speed * dt;
            particle.angle += dt * 0.15;
            alive.push(particle);
        }
        self.particles = alive;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn appearance() -> Appearance {
        Appearance::default()
    }

    fn viewport() -> Viewport {
        Viewport {
            width: 800.0,
            height: 600.0,
        }
    }

    #[test]
    fn palette_one_color_is_solid() {
        let mut app = appearance();
        app.colors = vec!["#112233".to_string()];
        let colors = palette(&app, 5);
        assert_eq!(colors.len(), 5);
        assert!(colors
            .iter()
            .all(|c| *c == [0x11 as f64, 0x22 as f64, 0x33 as f64]));
    }

    #[test]
    fn palette_endpoints_match_stops() {
        let mut app = appearance();
        app.colors = vec!["#000000".to_string(), "#FFFFFF".to_string()];
        let colors = palette(&app, 3);
        assert_eq!(colors[0], [0.0, 0.0, 0.0]);
        assert_eq!(colors[2], [255.0, 255.0, 255.0]);
        assert_eq!(colors[1], [127.5, 127.5, 127.5]);
    }

    #[test]
    fn height_over_100_reaches_center() {
        let mut app = appearance();
        app.height = 200;
        let g = geometry(viewport(), &app);
        assert_eq!(g.radius_base, 0.0);
        assert!(g.max_len > 0.0);
    }

    #[test]
    fn height_under_100_keeps_base_outside_center() {
        let mut app = appearance();
        app.height = 50;
        let g = geometry(viewport(), &app);
        assert!(g.radius_base > 0.0);
        assert!(g.max_len > 0.0);
    }

    #[test]
    fn rotation_can_be_frozen() {
        let mut app = appearance();
        app.rotate = false;
        let mut scene = Scene::new(1);
        let bands = vec![0.5; app.count as usize];
        scene.update(0.05, viewport(), &bands, &app);
        scene.update(0.05, viewport(), &bands, &app);
        assert_eq!(scene.rotation(), 0.0);
    }

    #[test]
    fn rotation_advances_when_enabled() {
        let app = appearance();
        let mut scene = Scene::new(1);
        let bands = vec![0.5; app.count as usize];
        scene.update(0.05, viewport(), &bands, &app);
        assert!(scene.rotation() > 0.0);
    }

    #[test]
    fn bars_match_count_and_grow_with_value() {
        let mut app = appearance();
        app.count = 32;
        let mut scene = Scene::new(1);
        let quiet = vec![0.1; 32];
        scene.update(0.05, viewport(), &quiet, &app);
        let short = scene.bars()[0];
        let loud = vec![1.0; 32];
        scene.update(0.05, viewport(), &loud, &app);
        let tall = scene.bars()[0];
        assert_eq!(scene.bars().len(), 32);
        let short_len = (short.x1 - short.x0).hypot(short.y1 - short.y0);
        let tall_len = (tall.x1 - tall.x0).hypot(tall.y1 - tall.y0);
        assert!(tall_len > short_len);
    }

    #[test]
    fn burst_toggle_controls_beat_detection() {
        let mut app = appearance();
        app.count = 64;

        // 先安静一阵让参考线落低，再来一记"低频骤升"。
        let run = |burst: bool| {
            let mut app = app.clone();
            app.burst = burst;
            let mut scene = Scene::new(1);
            for _ in 0..10 {
                scene.update(0.05, viewport(), &vec![0.05; 64], &app);
            }
            for _ in 0..5 {
                scene.update(0.05, viewport(), &vec![1.0; 64], &app);
            }
            scene.beat_count()
        };

        assert_eq!(run(false), 0, "关掉鼓点后不该判定出鼓点");
        assert!(run(true) > 0, "低频骤升应当触发鼓点");
    }

    #[test]
    fn particles_expire() {
        let mut app = appearance();
        app.count = 64;
        app.burst = false;
        app.particle_life = 20; // 最短寿命
        let mut scene = Scene::new(1);
        for _ in 0..20 {
            scene.update(0.05, viewport(), &vec![1.0; 64], &app);
        }
        let before = scene.particles().len();
        assert!(before > 0, "应当有粒子逸出");
        // 让输入静音并跑足够久，粒子应逐渐消失。
        for _ in 0..400 {
            scene.update(0.05, viewport(), &vec![0.0; 64], &app);
        }
        assert!(scene.particles().len() < before);
    }

    #[test]
    fn resample_handles_thin_input() {
        let mut app = appearance();
        app.count = 8;
        let mut scene = Scene::new(1);
        scene.update(0.05, viewport(), &[0.5], &app);
        assert_eq!(scene.shown().len(), 8);
    }

    #[test]
    fn afterglow_zero_is_infinite() {
        assert!(afterglow_rate(0.0).is_infinite());
        assert!(afterglow_rate(100.0) < afterglow_rate(1.0));
    }

    #[test]
    fn links_connect_neighboring_tips() {
        let mut app = appearance();
        app.count = 8;
        app.link = true;
        let mut scene = Scene::new(7);
        scene.update(0.016, viewport(), &[0.5; 8], &app);
        let bars = scene.bars().to_vec();
        let links = scene.links();
        assert_eq!(links.len(), 8, "闭合一圈：每条都连到下一条");
        assert!((links[0].x0 - bars[0].x1).abs() < 1e-9);
        assert!((links[0].x1 - bars[1].x1).abs() < 1e-9);
        assert!(
            (links[7].x1 - bars[0].x1).abs() < 1e-9,
            "最后一条连回第一条"
        );
    }

    #[test]
    fn links_can_be_disabled() {
        let mut app = appearance();
        app.count = 8;
        app.link = false;
        let mut scene = Scene::new(7);
        scene.update(0.016, viewport(), &[0.5; 8], &app);
        assert!(scene.links().is_empty());
    }

    #[test]
    fn rotation_speed_follows_config() {
        let bands = [0.0; 8];
        let mut slow = appearance();
        slow.count = 8;
        slow.rotate_speed = 50;
        let mut fast = appearance();
        fast.count = 8;
        fast.rotate_speed = 200;
        let mut a = Scene::new(1);
        let mut b = Scene::new(1);
        a.update(0.05, viewport(), &bands, &slow);
        b.update(0.05, viewport(), &bands, &fast);
        assert!(b.rotation() > a.rotation());
    }
}
