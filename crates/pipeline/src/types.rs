#[derive(Debug, Clone, PartialEq)]
pub enum RotateAngle {
    R90,
    R180,
    R270,
    Random,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FlipDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Channel {
    Gray,
    Red,
    Green,
    Blue,
    Hue,
    Saturation,
    Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FilterKind {
    Rotate(RotateAngle),
    GaussianBlur {
        sigma: f32,
    },
    Brighten {
        value: i32,
    },
    Resize {
        w: u32,
        h: u32,
    },
    Flip(FlipDirection),
    ExtractChannel(Channel),
    Contrast {
        value: f32,
    },
    Saturation {
        value: f32,
    },
    Crop {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    Grayscale,
    Noise {
        intensity: f32,
    },
    Sharpness {
        amount: f32,
    },
}
