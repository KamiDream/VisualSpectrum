//! 余辉拖尾的「淡出」趟：把上一帧的场景乘一个衰减系数，铺满当前帧的离屏纹理。
//!
//! 为什么不用加法混合累积：条本身每帧都在同一位置以 alpha=1 重画，加法会一路加到
//! 过曝发白；这里改成「上一帧 × decay」打底，再用普通 alpha 混合把新条盖上去——
//! 条覆盖率高的地方被新帧替换，只有条退下去的地方留下按指数衰减的残影。
//!
//! UV 变换和 `bloom.wgsl` / `crt.wgsl` 保持一致（左上原点）。

struct FsIn {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
};

// 与 `shader.wgsl` 的 Uniforms 同一份布局（共用 group 0 的 uniform buffer）。
struct Uniforms {
    size: vec2f,
    decay: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var samp: sampler;
@group(1) @binding(1) var prev: texture_2d<f32>;

/// 量化地板：8-bit 纹理里 `value *= decay` 会卡在 1~5/255 的定点上（四舍五入回原值），
/// 于是粒子的残影永远不消失。每帧再减 ~0.75/255，保证一定能走到 0。
const QUANT_FLOOR: f32 = 0.003;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> FsIn {
    var corners = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    let p = corners[vi];
    var out: FsIn;
    out.pos = vec4f(p, 0.0, 1.0);
    out.uv = vec2f(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

@fragment
fn fs_fade(in: FsIn) -> @location(0) vec4f {
    // 纹理是预乘 alpha，rgb / a 一起衰减才是正确（否则残影会越来越「亮而不透明」）。
    let faded = textureSample(prev, samp, in.uv) * u.decay;
    return max(faded - vec4f(QUANT_FLOOR), vec4f(0.0));
}
