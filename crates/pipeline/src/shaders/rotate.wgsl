@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> p: RotateParams;

struct RotateParams {
    angle: u32,
    pad1: u32,
    pad2: u32,
    pad3: u32,
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dim_src = textureDimensions(src);
    let dim_dst = textureDimensions(dst);
    let x = gid.x;
    let y = gid.y;

    if x >= dim_dst.x || y >= dim_dst.y { return; }

    var src_x = x;
    var src_y = y;

    if p.angle == 1u {
        src_x = y;
        src_y = dim_src.y - 1u - x;
    } else if p.angle == 2u {
        src_x = dim_src.x - 1u - x;
        src_y = dim_src.y - 1u - y;
    } else if p.angle == 3u {
        src_x = dim_src.x - 1u - y;
        src_y = x;
    }

    let color = textureLoad(src, vec2<i32>(i32(src_x), i32(src_y)), 0);
    textureStore(dst, vec2<i32>(i32(x), i32(y)), color);
}
