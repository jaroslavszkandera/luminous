use crate::{Channel, FlipDirection, RotateAngle};
use image::{DynamicImage, GenericImageView};
use log::info;
use std::borrow::Cow;

// source: inspired by blog post: https://blog.redwarp.app/image-filters/ and github repo: https://github.com/redwarp/filters
const BLUR_SHADER: &str = include_str!("shaders/blur.wgsl");
const RESIZE_SHADER: &str = include_str!("shaders/resize.wgsl");
const ROTATE_SHADER: &str = include_str!("shaders/rotate.wgsl");
const BRIGHTEN_SHADER: &str = include_str!("shaders/brighten.wgsl");
const FLIP_SHADER: &str = include_str!("shaders/flip.wgsl");
const EXTRACT_CHANNEL_SHADER: &str = include_str!("shaders/extract_channel.wgsl");

pub struct GpuTexture {
    pub(crate) tex: wgpu::Texture,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl GpuTexture {
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
}

pub struct GpuProcessor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    blur_pipeline: wgpu::ComputePipeline,
    resize_pipeline: wgpu::ComputePipeline,
    rotate_pipeline: wgpu::ComputePipeline,
    brighten_pipeline: wgpu::ComputePipeline,
    flip_pipeline: wgpu::ComputePipeline,
    extract_channel_pipeline: wgpu::ComputePipeline,
}

impl GpuProcessor {
    pub async fn new() -> Option<Self> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok()?;

        let info = adapter.get_info();
        info!("GPU: {} ({:?})", info.name, info.backend);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await
            .ok()?;

        let blur_pipeline = make_pipeline(&device, BLUR_SHADER, "main");
        let resize_pipeline = make_pipeline(&device, RESIZE_SHADER, "main");
        let rotate_pipeline = make_pipeline(&device, ROTATE_SHADER, "main");
        let brighten_pipeline = make_pipeline(&device, BRIGHTEN_SHADER, "main");
        let flip_pipeline = make_pipeline(&device, FLIP_SHADER, "main");
        let extract_channel_pipeline = make_pipeline(&device, EXTRACT_CHANNEL_SHADER, "main");

        Some(Self {
            device,
            queue,
            blur_pipeline,
            resize_pipeline,
            rotate_pipeline,
            brighten_pipeline,
            flip_pipeline,
            extract_channel_pipeline,
        })
    }

    pub fn upload(&self, img: &DynamicImage) -> GpuTexture {
        let (w, h) = img.dimensions();
        let rgba = img.to_rgba8();
        let tex = upload_tex(&self.device, &self.queue, &rgba, w, h);
        GpuTexture {
            tex,
            width: w,
            height: h,
        }
    }

    pub fn download(&self, gt: &GpuTexture) -> DynamicImage {
        readback(&self.device, &self.queue, &gt.tex, gt.width, gt.height)
    }

    pub fn blur_gpu(&self, src: &GpuTexture, sigma: f32) -> GpuTexture {
        let kernel_half_size = (sigma * 2.0).ceil() as u32;
        let (w, h) = (src.width, src.height);

        let mid = storage_tex(&self.device, w, h);
        self.blur_pass(&src.tex, &mid, w, h, kernel_half_size, 0);

        let dst = storage_tex(&self.device, w, h);
        self.blur_pass(&mid, &dst, w, h, kernel_half_size, 1);

        GpuTexture {
            tex: dst,
            width: w,
            height: h,
        }
    }

    fn blur_pass(
        &self,
        src: &wgpu::Texture,
        dst: &wgpu::Texture,
        w: u32,
        h: u32,
        kernel_half_size: u32,
        axis: u32,
    ) {
        #[repr(C)]
        #[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy)]
        struct Params {
            kernel_half_size: u32,
            axis: u32,
            width: u32,
            height: u32,
        }

        let ub = uniform_buf(
            &self.device,
            bytemuck::bytes_of(&Params {
                kernel_half_size,
                axis,
                width: w,
                height: h,
            }),
        );
        let bg = bind_group(&self.device, &self.blur_pipeline, src, dst, &ub);
        dispatch(&self.device, &self.queue, &self.blur_pipeline, &bg, w, h);
    }

    pub fn resize_gpu(&self, src: &GpuTexture, dst_w: u32, dst_h: u32) -> GpuTexture {
        #[repr(C)]
        #[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy)]
        struct Params {
            src_w: u32,
            src_h: u32,
            dst_w: u32,
            dst_h: u32,
        }

        let dst = storage_tex(&self.device, dst_w, dst_h);
        let ub = uniform_buf(
            &self.device,
            bytemuck::bytes_of(&Params {
                src_w: src.width,
                src_h: src.height,
                dst_w,
                dst_h,
            }),
        );
        let bg = bind_group(&self.device, &self.resize_pipeline, &src.tex, &dst, &ub);
        dispatch(
            &self.device,
            &self.queue,
            &self.resize_pipeline,
            &bg,
            dst_w,
            dst_h,
        );

        GpuTexture {
            tex: dst,
            width: dst_w,
            height: dst_h,
        }
    }

    pub fn rotate_gpu(&self, src: &GpuTexture, angle: RotateAngle) -> GpuTexture {
        let (dst_w, dst_h) = match angle {
            RotateAngle::R90 | RotateAngle::R270 => (src.height, src.width),
            _ => (src.width, src.height),
        };

        #[repr(C)]
        #[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy)]
        struct Params {
            angle: u32,
            _pad: [u32; 3],
        }

        let angle_val = match angle {
            RotateAngle::R90 => 1,
            RotateAngle::R180 => 2,
            RotateAngle::R270 => 3,
            _ => 0,
        };

        let dst = storage_tex(&self.device, dst_w, dst_h);
        let ub = uniform_buf(
            &self.device,
            bytemuck::bytes_of(&Params {
                angle: angle_val,
                _pad: [0; 3],
            }),
        );
        let bg = bind_group(&self.device, &self.rotate_pipeline, &src.tex, &dst, &ub);
        dispatch(
            &self.device,
            &self.queue,
            &self.rotate_pipeline,
            &bg,
            dst_w,
            dst_h,
        );

        GpuTexture {
            tex: dst,
            width: dst_w,
            height: dst_h,
        }
    }

    pub fn brighten_gpu(&self, src: &GpuTexture, value: i32) -> GpuTexture {
        #[repr(C)]
        #[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy)]
        struct Params {
            value: i32,
            _pad: [i32; 3],
        }

        let dst = storage_tex(&self.device, src.width, src.height);
        let ub = uniform_buf(
            &self.device,
            bytemuck::bytes_of(&Params {
                value,
                _pad: [0; 3],
            }),
        );
        let bg = bind_group(&self.device, &self.brighten_pipeline, &src.tex, &dst, &ub);
        dispatch(
            &self.device,
            &self.queue,
            &self.brighten_pipeline,
            &bg,
            src.width,
            src.height,
        );

        GpuTexture {
            tex: dst,
            width: src.width,
            height: src.height,
        }
    }

    pub fn flip_gpu(&self, src: &GpuTexture, dir: FlipDirection) -> GpuTexture {
        #[repr(C)]
        #[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy)]
        struct Params {
            dir: u32,
            _pad: [u32; 3],
        }

        let dir_val = match dir {
            FlipDirection::Horizontal => 0,
            FlipDirection::Vertical => 1,
        };

        let dst = storage_tex(&self.device, src.width, src.height);
        let ub = uniform_buf(
            &self.device,
            bytemuck::bytes_of(&Params {
                dir: dir_val,
                _pad: [0; 3],
            }),
        );
        let bg = bind_group(&self.device, &self.flip_pipeline, &src.tex, &dst, &ub);
        dispatch(
            &self.device,
            &self.queue,
            &self.flip_pipeline,
            &bg,
            src.width,
            src.height,
        );

        GpuTexture {
            tex: dst,
            width: src.width,
            height: src.height,
        }
    }

    pub fn extract_channel_gpu(&self, src: &GpuTexture, channel: Channel) -> GpuTexture {
        #[repr(C)]
        #[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy)]
        struct Params {
            channel: u32,
            _pad: [u32; 3],
        }

        let chan_val = match channel {
            Channel::Gray => 0,
            Channel::Red => 1,
            Channel::Green => 2,
            Channel::Blue => 3,
            Channel::Hue => 4,
            Channel::Saturation => 5,
            Channel::Value => 6,
        };

        let dst = storage_tex(&self.device, src.width, src.height);
        let ub = uniform_buf(
            &self.device,
            bytemuck::bytes_of(&Params {
                channel: chan_val,
                _pad: [0; 3],
            }),
        );
        let bg = bind_group(
            &self.device,
            &self.extract_channel_pipeline,
            &src.tex,
            &dst,
            &ub,
        );
        dispatch(
            &self.device,
            &self.queue,
            &self.extract_channel_pipeline,
            &bg,
            src.width,
            src.height,
        );

        GpuTexture {
            tex: dst,
            width: src.width,
            height: src.height,
        }
    }

    pub fn blur(&self, img: DynamicImage, sigma: f32) -> DynamicImage {
        let src = self.upload(&img);
        let dst = self.blur_gpu(&src, sigma);
        self.download(&dst)
    }

    pub fn resize(&self, img: DynamicImage, dst_w: u32, dst_h: u32) -> DynamicImage {
        let src = self.upload(&img);
        let dst = self.resize_gpu(&src, dst_w, dst_h);
        self.download(&dst)
    }
}

fn make_pipeline(device: &wgpu::Device, src: &str, entry: &str) -> wgpu::ComputePipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(src)),
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

fn upload_tex(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rgba: &image::RgbaImage,
    w: u32,
    h: u32,
) -> wgpu::Texture {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: extent(w, h),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        tex.as_image_copy(),
        rgba.as_raw(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * w),
            rows_per_image: None,
        },
        extent(w, h),
    );
    tex
}

fn storage_tex(device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: extent(w, h),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn uniform_buf(device: &wgpu::Device, data: &[u8]) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: data,
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn bind_group(
    device: &wgpu::Device,
    pipeline: &wgpu::ComputePipeline,
    input: &wgpu::Texture,
    output: &wgpu::Texture,
    uniform: &wgpu::Buffer,
) -> wgpu::BindGroup {
    let iv = input.create_view(&Default::default());
    let ov = output.create_view(&Default::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&iv),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&ov),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniform.as_entire_binding(),
            },
        ],
    })
}

fn dispatch(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    w: u32,
    h: u32,
) {
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_compute_pass(&Default::default());
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.dispatch_workgroups((w + 15) / 16, (h + 15) / 16, 1);
    }
    queue.submit(std::iter::once(enc.finish()));
}

fn readback(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    w: u32,
    h: u32,
) -> DynamicImage {
    let row_pitch = ((4 * w + 255) / 256) * 256;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row_pitch * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_pitch),
                rows_per_image: None,
            },
        },
        extent(w, h),
    );
    queue.submit(std::iter::once(enc.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();

    let raw = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((4 * w * h) as usize);
    for row in 0..h {
        let start = (row * row_pitch) as usize;
        pixels.extend_from_slice(&raw[start..start + (4 * w) as usize]);
    }
    drop(raw);
    staging.unmap();

    DynamicImage::ImageRgba8(
        image::RgbaImage::from_raw(w, h, pixels).expect("readback buffer mismatch"),
    )
}

fn extent(w: u32, h: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    }
}

// TODO: tests for every operation and full pipeline
#[cfg(test)]
mod tests {
    use super::*;
    use image::imageops;

    fn test_image(w: u32, h: u32) -> DynamicImage {
        let buf = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([
                ((x * 3) % 256) as u8,
                ((y * 3) % 256) as u8,
                ((x + y) % 256) as u8,
                255,
            ])
        });
        DynamicImage::ImageRgba8(buf)
    }

    fn calculate_mse(img1: &DynamicImage, img2: &DynamicImage) -> f32 {
        let (w, h) = img1.dimensions();
        assert_eq!((w, h), img2.dimensions());

        let mut error = 0.0;
        let buf1 = img1.to_rgba8();
        let buf2 = img2.to_rgba8();

        for (p1, p2) in buf1.pixels().zip(buf2.pixels()) {
            for c in 0..4 {
                let diff = p1[c] as f32 - p2[c] as f32;
                error += diff * diff;
            }
        }
        error / (w as f32 * h as f32 * 4.0)
    }

    #[test]
    fn test_compare_blur() {
        let img = test_image(128, 128);
        let sigma = 3.0;

        let cpu_blurred = imageops::blur(&img, sigma);
        let cpu_blurred_dyn = DynamicImage::ImageRgba8(cpu_blurred);

        let processor = pollster::block_on(GpuProcessor::new()).unwrap();
        let gpu_blurred = processor.blur(img, sigma);

        let mse = calculate_mse(&cpu_blurred_dyn, &gpu_blurred);
        assert!(mse < 25.0, "Blur MSE too high: {}", mse);
    }

    #[test]
    fn test_compare_resize() {
        let img = test_image(128, 128);
        let dst_w = 64;
        let dst_h = 64;

        let cpu_resized = imageops::resize(&img, dst_w, dst_h, imageops::FilterType::Nearest);
        let cpu_resized_dyn = DynamicImage::ImageRgba8(cpu_resized);

        let processor = pollster::block_on(GpuProcessor::new()).unwrap();
        let gpu_resized = processor.resize(img, dst_w, dst_h);

        let mse = calculate_mse(&cpu_resized_dyn, &gpu_resized);
        assert!(mse < 25.0, "Resize MSE too high: {}", mse);
    }
}
