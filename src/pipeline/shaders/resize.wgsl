// Bilinear resize.

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> p: ResizeParams;

struct ResizeParams {
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if x >= p.dst_width || y >= p.dst_height { return; }

    let u = (f32(x) + 0.5) / f32(p.dst_width) * f32(p.src_width) - 0.5;
    let v = (f32(y) + 0.5) / f32(p.dst_height) * f32(p.src_height) - 0.5;

    let x0 = clamp(i32(floor(u)), 0, i32(p.src_width) - 1);
    let y0 = clamp(i32(floor(v)), 0, i32(p.src_height) - 1);
    let x1 = clamp(x0 + 1, 0, i32(p.src_width) - 1);
    let y1 = clamp(y0 + 1, 0, i32(p.src_height) - 1);

    let fx = fract(u);
    let fy = fract(v);

    let c = mix(
        mix(textureLoad(src, vec2<i32>(x0, y0), 0),
            textureLoad(src, vec2<i32>(x1, y0), 0), fx),
        mix(textureLoad(src, vec2<i32>(x0, y1), 0),
            textureLoad(src, vec2<i32>(x1, y1), 0), fx),
        fy
    );

    textureStore(dst, vec2<i32>(i32(x), i32(y)), c);
}
