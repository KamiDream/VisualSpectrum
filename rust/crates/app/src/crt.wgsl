//! CRT 合成：桶形畸变 + RGB 分色 + 叠加 bloom + 背景色。

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

struct Post {
    //: 背景色（**预乘 alpha**）。
    bg: vec4f,
    //: (桶形强度, RGB 分色强度, bloom 强度, 预留)
    params: vec4f,
};

@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var scene_tex: texture_2d<f32>;
@group(0) @binding(2) var bloom_tex: texture_2d<f32>;
@group(0) @binding(3) var<uniform> post: Post;

@fragment
fn fs_crt(in: FsIn) -> @location(0) vec4f {
    let d = in.uv - vec2f(0.5);
    let r2 = dot(d, d);

    // 桶形：越靠边采样点越往中心收 → 画面边缘向外鼓。
    let uv = vec2f(0.5) + d * (1.0 - post.params.x * r2 * 4.0);
    // RGB 分色：R 往外偏、B 往里偏，越靠边越明显。
    let off = d * (post.params.y * r2 * 8.0);

    let sr = textureSample(scene_tex, samp, uv + off);
    let sg = textureSample(scene_tex, samp, uv);
    let sb = textureSample(scene_tex, samp, uv - off);
    var scene = vec3f(sr.r, sg.g, sb.b);
    let scene_alpha = max(sg.a, max(sr.a, sb.a));

    // 畸变后落在画面外：当作透明（**bloom 也要一起清**，否则边缘会拖出一圈光）。
    let outside = uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0;
    let bloom = textureSample(bloom_tex, samp, uv).rgb * post.params.z;
    var color = scene + bloom;
    var alpha = scene_alpha;
    if (outside) {
        color = vec3f(0.0);
        alpha = 0.0;
    }

    // 预乘合成：背景在后、场景在前。
    let out_rgb = color + post.bg.rgb * (1.0 - alpha);
    let out_a = clamp(alpha + post.bg.a * (1.0 - alpha), 0.0, 1.0);
    return vec4f(out_rgb, out_a);
}
