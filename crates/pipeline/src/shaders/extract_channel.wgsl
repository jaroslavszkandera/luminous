@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> p: ChannelParams;

struct ChannelParams {
    channel: u32,
    pad1: u32,
    pad2: u32,
    pad3: u32,
}

fn rgb2hsv(c: vec3<f32>) -> vec3<f32> {
    let v = max(c.r, max(c.g, c.b));
    let cmin = min(c.r, min(c.g, c.b));
    let delta = v - cmin;

    var h = 0.0;
    if delta == 0.0 {
        h = 0.0;
    } else if v == c.r {
        h = (c.g - c.b) / delta;
    } else if v == c.g {
        h = 2.0 + (c.b - c.r) / delta;
    } else {
        h = 4.0 + (c.r - c.g) / delta;
    }

    h = h * 60.0;
    if h < 0.0 {
        h = h + 360.0;
    }

    var s = 0.0;
    if v != 0.0 {
        s = delta / v;
    }

    return vec3<f32>(h, s, v);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dim = textureDimensions(src);
    if gid.x >= dim.x || gid.y >= dim.y { return; }

    let color = textureLoad(src, vec2<i32>(gid.xy), 0);
    var out_val = 0.0;

    if p.channel == 0u {
        out_val = 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
    } else if p.channel == 1u {
        out_val = color.r;
    } else if p.channel == 2u {
        out_val = color.g;
    } else if p.channel == 3u {
        out_val = color.b;
    } else {
        let hsv = rgb2hsv(color.rgb);
        if p.channel == 4u {
            out_val = hsv.x / 360.0;
        } else if p.channel == 5u {
            out_val = hsv.y;
        } else if p.channel == 6u {
            out_val = hsv.z;
        }
    }

    textureStore(dst, vec2<i32>(gid.xy), vec4<f32>(out_val, out_val, out_val, color.a));
}
