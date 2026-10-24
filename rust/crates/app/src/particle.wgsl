//! 散射粒子：一个实例 = 一颗小圆点，展开成 billboard 四边形，片元里做圆形软边。
//!
//! 用**加法混合**而不是 alpha 覆盖：粒子是光点，叠在一起应该更亮（和真实荧光粉一致），
//! 也让它们在余辉缓冲里留下更自然的残影。

struct Uniforms {
    size: vec2f,
    decay: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct InstanceIn {
    @location(0) center: vec2f,
    @location(1) radius: f32,
    @location(2) alpha: f32,
    @location(3) color: vec4f,
};

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) local: vec2f,
    @location(1) alpha: f32,
    @location(2) color: vec4f,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: InstanceIn) -> VsOut {
    var corners = array<vec2f, 6>(
        vec2f(-1.0, -1.0),
        vec2f(1.0, -1.0),
        vec2f(1.0, 1.0),
        vec2f(-1.0, -1.0),
        vec2f(1.0, 1.0),
        vec2f(-1.0, 1.0),
    );
    let c = corners[vi];

    // 四边形比粒子本身大 2.2 倍，多出来的部分用来画柔和光晕。
    let pixel = inst.center + c * (inst.radius * 2.2);
    let ndc = vec2f(pixel.x / u.size.x * 2.0 - 1.0, 1.0 - pixel.y / u.size.y * 2.0);

    var out: VsOut;
    out.pos = vec4f(ndc, 0.0, 1.0);
    out.local = c;
    out.alpha = inst.alpha;
    out.color = inst.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4f {
    let d = length(in.local);
    // 中心 0.45 以内几乎实心，外面指数式淡出到 0。
    let falloff = smoothstep(1.0, 0.45, d);
    let a = in.alpha * falloff;
    // 预乘：rgb 乘 alpha，配合加法混合。
    return vec4f(in.color.rgb * a, a);
}
