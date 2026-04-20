@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> p: BrightenParams;

struct BrightenParams {
    value: i32,
    pad1: i32,
    pad2: i32,
    pad3: i32,
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dim = textureDimensions(src);
    if gid.x >= dim.x || gid.y >= dim.y { return; }

    let color = textureLoad(src, vec2<i32>(gid.xy), 0);
    let factor = f32(p.value) / 255.0;

    var new_color = color.rgb + vec3<f32>(factor);
    new_color = clamp(new_color, vec3<f32>(0.0), vec3<f32>(1.0));

    textureStore(dst, vec2<i32>(gid.xy), vec4<f32>(new_color, color.a));
}
