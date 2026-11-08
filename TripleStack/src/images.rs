use image::{Rgba, RgbaImage};
use std::{borrow::Cow, f64::consts::PI};

pub fn stack_number(number: &RgbaImage, circle: &RgbaImage) -> RgbaImage {
    let mut canvas = RgbaImage::new(circle.width(), circle.height());
    paste_centered(&mut canvas, number);
    paste_centered(&mut canvas, circle);
    canvas
}

// Pillow's paste(..., mask=image) interpolates all four channels, including alpha.
fn paste_centered(canvas: &mut RgbaImage, source: &RgbaImage) {
    let x = (i64::from(canvas.width()) - i64::from(source.width())).div_euclid(2);
    let y = (i64::from(canvas.height()) - i64::from(source.height())).div_euclid(2);
    for (sx, sy, pixel) in source.enumerate_pixels() {
        let dx = x + i64::from(sx);
        let dy = y + i64::from(sy);
        if dx >= 0 && dy >= 0 && dx < i64::from(canvas.width()) && dy < i64::from(canvas.height()) {
            let dest = canvas.get_pixel_mut(dx as u32, dy as u32);
            let mask = u32::from(pixel[3]);
            for channel in 0..4 {
                dest[channel] = ((u32::from(dest[channel]) * (255 - mask)
                    + u32::from(pixel[channel]) * mask
                    + 127)
                    / 255) as u8;
            }
        }
    }
}

pub fn composite(base: &mut RgbaImage, top: &RgbaImage) {
    // Match Pillow's 7-bit fixed-point source-over rounding.
    const SCALE: u32 = 128;
    for (dst, src) in base.pixels_mut().zip(top.pixels()) {
        let source_alpha = u32::from(src[3]);
        if source_alpha == 0 {
            continue;
        }
        let alpha_sum = source_alpha * 255 + u32::from(dst[3]) * (255 - source_alpha);
        let src_weight = source_alpha * 255 * 255 * SCALE / alpha_sum;
        let dst_weight = 255 * SCALE - src_weight;
        for channel in 0..3 {
            let sum = u32::from(src[channel]) * src_weight + u32::from(dst[channel]) * dst_weight;
            dst[channel] = (divide_255(sum + 128 * SCALE) / SCALE) as u8;
        }
        dst[3] = ((alpha_sum + 127) / 255) as u8;
    }
}

fn divide_255(value: u32) -> u32 {
    (value + (value >> 8)) >> 8
}

// Match Pillow's Lanczos resampling of premultiplied 8-bit RGBA.
pub fn resize_circle(source: &RgbaImage, (width, height): (u32, u32)) -> Cow<'_, RgbaImage> {
    if source.dimensions() == (width, height) {
        return Cow::Borrowed(source);
    }

    let mut premultiplied = source.clone();
    for pixel in premultiplied.pixels_mut() {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel.0[..3] {
            *channel = ((u32::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    let width_reduction = i64::from(source.width()) - i64::from(width);
    let height_reduction = i64::from(source.height()) - i64::from(height);
    let axes = if height_reduction > 0 && height_reduction > 2 * width_reduction {
        [(height, false), (width, true)]
    } else {
        [(width, true), (height, false)]
    };
    for (length, horizontal) in axes {
        let original = if horizontal {
            premultiplied.width()
        } else {
            premultiplied.height()
        };
        if original != length {
            premultiplied = resample_axis(&premultiplied, length, horizontal);
        }
    }
    for pixel in premultiplied.pixels_mut() {
        let alpha = u32::from(pixel[3]);
        if (1..255).contains(&alpha) {
            for channel in &mut pixel.0[..3] {
                *channel = (u32::from(*channel) * 255 / alpha).min(255) as u8;
            }
        }
    }
    Cow::Owned(premultiplied)
}

const WEIGHT_BITS: u32 = 22;

fn resample_axis(source: &RgbaImage, length: u32, horizontal: bool) -> RgbaImage {
    let (input_length, width, height) = if horizontal {
        (source.width(), length, source.height())
    } else {
        (source.height(), source.width(), length)
    };

    let scale = f64::from(input_length) / f64::from(length);
    let support = 3.0 * scale.max(1.0);
    let inverse_filter_scale = 1.0 / scale.max(1.0);
    let weights: Vec<_> = (0..length)
        .map(|index| {
            let center = (f64::from(index) + 0.5) * scale;
            let first = (center - support + 0.5).max(0.0) as u32;
            let end = ((center + support + 0.5) as u32).min(input_length);
            let weights: Vec<f64> = (first..end)
                .map(|sample| lanczos((f64::from(sample) - center + 0.5) * inverse_filter_scale))
                .collect();
            let total: f64 = weights.iter().sum();
            let quantized: Vec<i32> = weights
                .into_iter()
                .map(|weight| (weight / total * f64::from(1 << WEIGHT_BITS)).round() as i32)
                .collect();
            (first, quantized)
        })
        .collect();

    RgbaImage::from_fn(width, height, |x, y| {
        let index = if horizontal { x } else { y };
        let (first, weights) = &weights[index as usize];
        let mut sums = [1 << (WEIGHT_BITS - 1); 4];
        for (offset, weight) in weights.iter().enumerate() {
            let sample = first + offset as u32;
            let pixel = if horizontal {
                source.get_pixel(sample, y)
            } else {
                source.get_pixel(x, sample)
            };
            for (sum, channel) in sums.iter_mut().zip(pixel.0) {
                *sum += i32::from(channel) * weight;
            }
        }
        Rgba(sums.map(|sum| (sum >> WEIGHT_BITS).clamp(0, 255) as u8))
    })
}

fn lanczos(x: f64) -> f64 {
    fn sinc(x: f64) -> f64 {
        if x == 0.0 {
            1.0
        } else {
            let angle = x * PI;
            angle.sin() / angle
        }
    }
    if (-3.0..3.0).contains(&x) {
        sinc(x) * sinc(x / 3.0)
    } else {
        0.0
    }
}
