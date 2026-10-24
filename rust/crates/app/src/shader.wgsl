//! 条状顶点的着色器：每条频谱条 = 一个实例，展开成两个三角形（细长胶囊的矩形近似）。
//!
//! 坐标是**屏幕像素**（和 `vs-scene` 给的几何同一个空间），这里换算成 NDC。

struct Uniforms {
    size: vec2f,
    // 余辉衰减系数（本着色器不用，仅保持与 Rust 侧 ViewportUniform 的布局一致）。
    decay: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct InstanceIn {
    @location(0) p0: vec2f,
    @location(1) p1: vec2f,
    @location(2) width: f32,
    @location(3) color: vec4f,
};

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) color: vec4f,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: InstanceIn) -> VsOut {
    // 沿着 p0→p1 的矩形：c.x ∈ {0,1} 表示在线段的哪一端，c.y ∈ {-1,1} 表示法线方向。
    var corners = array<vec2f, 6>(
        vec2f(0.0, -1.0),
        vec2f(1.0, -1.0),
        vec2f(1.0, 1.0),
        vec2f(0.0, -1.0),
        vec2f(1.0, 1.0),
        vec2f(0.0, 1.0),
    );
    let c = corners[vi];

    let delta = inst.p1 - inst.p0;
    let len = length(delta);
    let dir = select(vec2f(0.0, 0.0), delta / max(len, 1e-6), len > 1e-6);
    let normal = vec2f(-dir.y, dir.x) * (inst.width * 0.5);

    let pixel = mix(inst.p0, inst.p1, c.x) + normal * c.y;
    // 像素 → NDC（y 轴翻转：屏幕向下、NDC 向上）。
    let ndc = vec2f(pixel.x / u.size.x * 2.0 - 1.0, 1.0 - pixel.y / u.size.y * 2.0);

    var out: VsOut;
    out.pos = vec4f(ndc, 0.0, 1.0);
    out.color = inst.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4f {
    return in.color;
}
