use crate::{
    Channel as SlintChannel, FlipDirection as SlintFlipDir, PipelineStep, PipelineStepKind,
    RotateAngle as SlintRotateAngle,
};
use luminous_pipeline::types::{Channel, FilterKind, FlipDirection, RotateAngle};

pub fn to_filter_kind(step: &PipelineStep) -> FilterKind {
    match step.kind {
        PipelineStepKind::Rotate => FilterKind::Rotate(map_angle(step.rotate_angle)),
        PipelineStepKind::GaussianBlur => FilterKind::GaussianBlur {
            sigma: step.blur_sigma,
        },
        PipelineStepKind::Brighten => FilterKind::Brighten {
            value: step.brighten_value,
        },
        PipelineStepKind::Resize => FilterKind::Resize {
            w: step.resize_width.max(1) as u32,
            h: step.resize_height.max(1) as u32,
        },
        PipelineStepKind::Flip => FilterKind::Flip(map_flip(step.flip_direction)),
        PipelineStepKind::ExtractChannel => {
            FilterKind::ExtractChannel(map_channel(step.extract_channel))
        }
        PipelineStepKind::Contrast => FilterKind::Contrast {
            value: step.contrast_value,
        },
        PipelineStepKind::Saturation => FilterKind::Saturation {
            value: step.saturation_value,
        },
        PipelineStepKind::Crop => FilterKind::Crop {
            x: step.crop_x.max(0) as u32,
            y: step.crop_y.max(0) as u32,
            width: step.crop_width.max(1) as u32,
            height: step.crop_height.max(1) as u32,
        },
        PipelineStepKind::Grayscale => FilterKind::Grayscale,
        PipelineStepKind::Noise => FilterKind::Noise {
            intensity: step.noise_intensity,
        },
        PipelineStepKind::Sharpness => FilterKind::Sharpness {
            amount: step.sharpness_amount,
        },
    }
}

fn map_angle(a: SlintRotateAngle) -> RotateAngle {
    match a {
        SlintRotateAngle::R90 => RotateAngle::R90,
        SlintRotateAngle::R180 => RotateAngle::R180,
        SlintRotateAngle::R270 => RotateAngle::R270,
        SlintRotateAngle::Random => RotateAngle::Random,
    }
}

fn map_flip(d: SlintFlipDir) -> FlipDirection {
    match d {
        SlintFlipDir::Horizontal => FlipDirection::Horizontal,
        SlintFlipDir::Vertical => FlipDirection::Vertical,
    }
}

fn map_channel(c: SlintChannel) -> Channel {
    match c {
        SlintChannel::Gray => Channel::Gray,
        SlintChannel::Red => Channel::Red,
        SlintChannel::Green => Channel::Green,
        SlintChannel::Blue => Channel::Blue,
        SlintChannel::Hue => Channel::Hue,
        SlintChannel::Saturation => Channel::Saturation,
        SlintChannel::Value => Channel::Value,
    }
}
