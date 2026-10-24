//! 廉价 bloom：两趟可分离高斯模糊（水平 + 垂直），在 1/4 分辨率上把频谱糊成柔和光晕。
//!
//! 为什么不用单趟 9 抽头：1/4 分辨率下条已经比像素还细，稀疏采样会产生「车轮效应」——
//! 慢旋转看起来变成快速反向旋转（用户实测遇到过的白环）。两趟 + 更宽的核能把它抹平。

struct FsIn {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> FsIn {
    var corners = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    let p = corners[vi];
    var out: FsIn;
    out.pos = vec4f(p, 0.0, 1.0);
    out.uv = vec2f(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var src: texture_2d<f32>;

// 9 抽头高斯权重（和为 1）。
fn blur9(uv: vec2f, step: vec2f) -> vec4f {
    var weights = array<f32, 9>(
        0.0276, 0.0663, 0.1238, 0.1802, 0.2042, 0.1802, 0.1238, 0.0663, 0.0276,
    );
    var sum = vec4f(0.0);
    for (var i: i32 = -4; i <= 4; i = i + 1) {
        sum = sum + textureSample(src, samp, uv + step * f32(i)) * weights[i + 4];
    }
    return sum;
}

@fragment
fn fs_blur_h(in: FsIn) -> @location(0) vec4f {
    let texel = 1.0 / vec2f(textureDimensions(src));
    // 步长 1.5 texel：9 抽头覆盖约 ±6 texel（≈ 全分辨率 24px），足以跨过条间距。
    return blur9(in.uv, vec2f(texel.x * 1.5, 0.0));
}

@fragment
fn fs_blur_v(in: FsIn) -> @location(0) vec4f {
    let texel = 1.0 / vec2f(textureDimensions(src));
    return blur9(in.uv, vec2f(0.0, texel.y * 1.5));
}
