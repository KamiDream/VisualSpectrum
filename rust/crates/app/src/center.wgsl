//! 中心组件：一圈套一圈的**同心环**（不填充）+ X 形油光 + 自定义色标签。
//!
//! 全部在一个 fragment 里按像素距离算：比画 10 个圆环实例省事，也不会因为三角函数
//! 分段数不够出现多边形边。着色器只负责「中心这块正方形」，像素代价很小。
//!
//! 混合用**预乘 alpha**：光环输出 `vec4(rgb, 0)` = 纯加法（残影/辉光不会被 alpha 压暗），
//! 标签输出 `vec4(rgb, 1)` = 不透明覆盖。

struct Viewport {
    size: vec2f,
    decay: f32,
    _pad: f32,
};

struct Center {
    center: vec2f,
    radius: f32,
    label_radius: f32,
    spin: f32,
    line_width: f32,
    ring_inner: f32,
    ring_outer: f32,
    /// 低频能量（0~1），让中心和旧版一样轻微起伏。
    bass: f32,
    /// 光晕强度倍数（配置 `center_glow / 100`）。
    glow: f32,
    stop_count: u32,
    _pad0: u32,
    /// 标签渐变的色标（最多 8 个，多色 = 渐变）。
    stops: array<vec4f, 8>,
    /// 10 个同心环的颜色（和条同一套 palette 规则）。
    ring_colors: array<vec4f, 10>,
};

@group(0) @binding(0) var<uniform> vp: Viewport;
@group(1) @binding(0) var<uniform> u: Center;

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) pixel: vec2f,
};

const RING_COUNT: u32 = 10u;
const STOPS_MAX: u32 = 8u;
const PI: f32 = 3.14159265359;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var corners = array<vec2f, 6>(
        vec2f(-1.0, -1.0),
        vec2f(1.0, -1.0),
        vec2f(1.0, 1.0),
        vec2f(-1.0, -1.0),
        vec2f(1.0, 1.0),
        vec2f(-1.0, 1.0),
    );
    let c = corners[vi];
    // 四边形比中心圆大 35%，给外圈光晕留空间。
    let pixel = u.center + c * (u.radius * 1.35);
    let ndc = vec2f(pixel.x / vp.size.x * 2.0 - 1.0, 1.0 - pixel.y / vp.size.y * 2.0);

    var out: VsOut;
    out.pos = vec4f(ndc, 0.0, 1.0);
    out.pixel = pixel;
    return out;
}

/// 沿对角线的多段渐变（和 `createLinearGradient(c-lr, c-lr, c+lr, c+lr)` 等价）。
fn gradient_color(t: f32) -> vec3f {
    let n = max(u.stop_count, 1u);
    var col = u.stops[0].rgb;
    for (var i: u32 = 1u; i < STOPS_MAX; i = i + 1u) {
        if (i < n) {
            let a = f32(i - 1u) / f32(n - 1u);
            let b = f32(i) / f32(n - 1u);
            if (t >= a) {
                let local = clamp((t - a) / max(b - a, 1e-5), 0.0, 1.0);
                col = mix(u.stops[i - 1u].rgb, u.stops[i].rgb, local);
            }
        }
    }
    return col;
}

/// 一条穿过中心的亮带：`angle` 是渐变轴，亮带与它垂直；两条差 π/2 就是 X。
/// 基准强度 0.09：再高就会盖过环线、显得刺眼（用户反馈过亮）。
fn sheen_band(q: vec2f, angle: f32) -> f32 {
    let n = vec2f(cos(angle), sin(angle));
    let t = dot(q, n) / max(u.radius, 1.0);
    return exp(-(t * t) / (0.055 * 0.055)) * 0.09 * u.glow;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4f {
    let q = in.pixel - u.center;
    let d = length(q);
    let r = max(u.radius, 1.0);

    // ---- 标签（不透明，画在环的内侧）----
    if (d < u.label_radius + 1.5) {
        let lr = max(u.label_radius, 1.0);
        let t = clamp((q.x + q.y) / (4.0 * lr) + 0.5, 0.0, 1.0);
        var col = gradient_color(t);
        // 外圈白描边
        if (abs(d - u.label_radius) < u.line_width * 0.5) {
            col = mix(col, vec3f(1.0), 0.30);
        }
        // 中心孔
        if (d < r * 0.05) {
            col = vec3f(8.0, 9.0, 12.0) / 255.0;
        }
        // 边缘 1.5px 抗锯齿
        let edge = clamp(u.label_radius + 1.5 - d, 0.0, 1.0);
        return vec4f(col * edge, edge);
    }

    // ---- 光晕（加法）----
    let halo_t = 1.0 - clamp((d - r * 0.25) / (r * 1.05), 0.0, 1.0);
    var glow = vec3f(0.47, 0.82, 1.0) * (pow(halo_t, 1.6) * (0.09 + u.bass * 0.10) * u.glow);

    // ---- 同心环（不填充，颜色按 palette 铺开）----
    for (var i: u32 = 0u; i < RING_COUNT; i = i + 1u) {
        let t = f32(i) / f32(RING_COUNT - 1u);
        let rr = mix(u.ring_inner, u.ring_outer, t);
        // ±line_width/2 的细线，边缘 1px 柔化。
        let mask = 1.0 - smoothstep(u.line_width * 0.5, u.line_width * 0.5 + 1.0, abs(d - rr));
        let c = mix(u.ring_colors[i].rgb, vec3f(1.0), 0.1);
        glow = glow + c * (mask * 0.55);
    }

    // ---- X 形油光：跟着自转走，只出现在「环带」上 ----
    let outside_label = smoothstep(u.label_radius * 1.02, u.label_radius * 1.25, d);
    let inside_outer = 1.0 - smoothstep(r * 0.86, r * 1.0, d);
    let spin = u.spin;
    let band = sheen_band(q, spin + PI * 0.25) + sheen_band(q, spin + PI * 0.75);
    glow = glow + vec3f(1.0) * (band * outside_label * inside_outer);

    // 预乘：alpha 取最大通道，保证 rgb <= alpha。和余辉叠在一起时稳态亮度不会无限累积
    // （纯加法配合「每帧只衰减 2%」会几十倍过曝）。
    let rgb = min(glow, vec3f(1.0));
    let alpha = max(rgb.r, max(rgb.g, rgb.b));
    return vec4f(rgb, alpha);
}
