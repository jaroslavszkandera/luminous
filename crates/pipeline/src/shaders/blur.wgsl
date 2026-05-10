// Gaussian blur — horizontal pass (bind_group 0)
// Must be run twice: once horizontally (axis=0), once vertically (axis=1).

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> p: BlurParams;

struct BlurParams {
    kernel_half_size: u32,
    axis: u32,  // 0 = horizontal, 1 = vertical
    width: u32,
    height: u32,
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if x >= p.width || y >= p.height { return; }

    let r = i32(p.kernel_half_size);
    let sigma = f32(r) * 0.5;
    let denom = 2.0 * sigma * sigma;

    var acc = vec4<f32>(0.0);
    var weight = 0.0;

    for (var i = -r; i <= r; i++) {
        let nx = clamp(i32(x) + i32(p.axis == 0u) * i, 0, i32(p.width) - 1);
        let ny = clamp(i32(y) + i32(p.axis == 1u) * i, 0, i32(p.height) - 1);
        let w = exp(-f32(i * i) / denom);
        acc += textureLoad(src, vec2<i32>(nx, ny), 0) * w;
        weight += w;
    }

    textureStore(dst, vec2<i32>(i32(x), i32(y)), acc / weight);
}
