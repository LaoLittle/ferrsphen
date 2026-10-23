use anyhow::Result;
use image::{ImageDecoder as _, ImageReader, RgbaImage, imageops::FilterType};
use std::path::Path;

pub const FRAMES: usize = 25;
pub const RASTER_SCALE: u32 = 2;
pub const COLUMNS: u32 = 5;
pub const ACTION_HEIGHT: f32 = 32.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardSize {
    pub width: f32,
    pub height: f32,
}
pub fn fitted(width: u32, height: u32) -> CardSize {
    if width == 0 || height == 0 {
        return CardSize {
            width: 240.,
            height: 180.,
        };
    }
    let scale = (288. / width as f32).min(300. / height as f32).min(1.);
    CardSize {
        width: (width as f32 * scale).max(1.),
        height: (height as f32 * scale).max(1.),
    }
}
pub fn angle(slot: usize) -> f32 {
    match slot {
        1 => -0.065,
        2 => 0.105,
        3 => -0.095,
        4 => 0.045,
        _ => 0.,
    }
}
pub struct CollapsedCards {
    pub width: f32,
    pub height: f32,
    pub centers: Vec<(f32, f32)>,
    pub sizes: Vec<CardSize>,
}

pub fn collapsed(sizes: &[CardSize]) -> CollapsedCards {
    let count = if sizes.len() <= 5 { sizes.len() } else { 3 };
    let mut layout = CollapsedCards {
        width: 0.,
        height: 0.,
        centers: Vec::new(),
        sizes: Vec::new(),
    };
    if sizes.len() <= 5 {
        layout.sizes = sizes
            .iter()
            .map(|size| {
                let scale = (144. / size.width).min(140. / size.height).min(1.);
                CardSize {
                    width: size.width * scale,
                    height: size.height * scale,
                }
            })
            .collect();
        let mut top = 20.;
        let offsets = [0., 8., 4., 0., 8.];
        for (slot, size) in layout.sizes.iter().enumerate() {
            let rotated = bounds(*size, angle(slot));
            let left = 20. + offsets[slot];
            layout
                .centers
                .push((left + rotated.width / 2., top + rotated.height / 2.));
            layout.width = layout.width.max(left + rotated.width + 20.);
            top += rotated.height + 4.;
            layout.height = top + 16.;
        }
    } else {
        layout.sizes.extend_from_slice(&sizes[..count]);
        let width = sizes
            .iter()
            .take(count)
            .map(|size| size.width)
            .fold(0., f32::max);
        let height = sizes
            .iter()
            .take(count)
            .map(|size| size.height)
            .fold(0., f32::max);
        for slot in 0..count {
            layout.centers.push((
                20. + width / 2. + slot as f32 * 8.,
                20. + height / 2. + slot as f32 * 8.,
            ));
        }
        layout.width = width + 56.;
        layout.height = height + 56.;
    }
    layout
}

pub fn frame(progress: f32) -> usize {
    (progress.clamp(0., 1.) * (FRAMES - 1) as f32).round() as usize
}
pub fn bounds(size: CardSize, angle: f32) -> CardSize {
    let (sine, cosine) = angle.sin_cos();
    CardSize {
        width: size.width * cosine.abs() + size.height * sine.abs(),
        height: size.height * cosine.abs() + size.width * sine.abs(),
    }
}

pub fn render(source: &Path, output: &Path, slot: usize, width: u32, height: u32) -> Result<()> {
    let mut reader = ImageReader::open(source)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = image::DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    let picture = image
        .resize_exact(
            width.max(1) * RASTER_SCALE,
            height.max(1) * RASTER_SCALE,
            FilterType::Lanczos3,
        )
        .to_rgba8();
    let (cell_width, cell_height) = (
        picture.width() + 40 * RASTER_SCALE,
        picture.height() + 40 * RASTER_SCALE,
    );
    let mut sheet = RgbaImage::new(cell_width * COLUMNS, cell_height * COLUMNS);
    for index in 0..FRAMES {
        let angle = angle(slot) * (1. - index as f32 / (FRAMES - 1) as f32);
        let card = tilt(&picture, angle);
        image::imageops::replace(
            &mut sheet,
            &card,
            i64::from(index as u32 % COLUMNS * cell_width),
            i64::from(index as u32 / COLUMNS * cell_height),
        );
    }
    sheet.save_with_format(output, image::ImageFormat::Png)?;
    Ok(())
}

fn tilt(picture: &RgbaImage, angle: f32) -> RgbaImage {
    let (width, height) = picture.dimensions();
    let padding = 40 * RASTER_SCALE;
    let mut output = RgbaImage::new(width + padding, height + padding);
    let (sine, cosine) = angle.sin_cos();
    let radius = ((8 * RASTER_SCALE) as f32).min(width.min(height) as f32 / 2.);
    for (x, y, pixel) in output.enumerate_pixels_mut() {
        let dx = x as f32 + 0.5 - (width + padding) as f32 / 2.;
        let dy = y as f32 + 0.5 - (height + padding) as f32 / 2.;
        let sx = cosine * dx + sine * dy + width as f32 / 2.;
        let sy = -sine * dx + cosine * dy + height as f32 / 2.;
        let qx = (sx - width as f32 / 2.).abs() - (width as f32 / 2. - radius);
        let qy = (sy - height as f32 / 2.).abs() - (height as f32 / 2. - radius);
        let distance = qx.max(0.).hypot(qy.max(0.)) + qx.max(qy).min(0.) - radius;
        let coverage = (0.5 - distance).clamp(0., 1.);
        if coverage == 0. {
            continue;
        }
        let sx = (sx - 0.5).clamp(0., width as f32 - 1.);
        let sy = (sy - 0.5).clamp(0., height as f32 - 1.);
        let (ix, iy) = (sx.floor() as u32, sy.floor() as u32);
        let (fx, fy) = (sx.fract(), sy.fract());
        let mut alpha = 0.;
        let mut color = [0.; 3];
        for (x, y, weight) in [
            (ix, iy, (1. - fx) * (1. - fy)),
            ((ix + 1).min(width - 1), iy, fx * (1. - fy)),
            (ix, (iy + 1).min(height - 1), (1. - fx) * fy),
            ((ix + 1).min(width - 1), (iy + 1).min(height - 1), fx * fy),
        ] {
            let sample = picture.get_pixel(x, y);
            let weighted_alpha = weight * sample[3] as f32 / 255.;
            alpha += weighted_alpha;
            for channel in 0..3 {
                color[channel] += sample[channel] as f32 * weighted_alpha;
            }
        }
        if alpha > 0. {
            for channel in 0..3 {
                pixel[channel] = (color[channel] / alpha).round() as u8;
            }
            pixel[3] = (alpha * coverage * 255.).round() as u8;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    #[test]
    fn fitting_keeps_aspect_and_small_native_sizes() {
        let landscape = super::fitted(1920, 1080);
        let portrait = super::fitted(1080, 1920);
        assert!((landscape.width / landscape.height - 1920. / 1080.).abs() < 0.001);
        assert!((portrait.width / portrait.height - 1080. / 1920.).abs() < 0.001);
        assert_ne!(landscape, portrait);
        assert_eq!(super::fitted(32, 24).width, 32.);
    }
    #[test]
    fn small_stacks_show_each_card_at_its_own_size_and_angle() {
        let sizes = [
            super::fitted(160, 90),
            super::fitted(80, 160),
            super::fitted(240, 120),
            super::fitted(120, 200),
            super::fitted(32, 24),
        ];
        for count in 2..=5 {
            let layout = super::collapsed(&sizes[..count]);
            assert_eq!(layout.centers.len(), count);
            let mut bottom = 0.;
            for (slot, native) in sizes.iter().take(count).enumerate() {
                let rotated = super::bounds(layout.sizes[slot], super::angle(slot));
                let (x, y) = layout.centers[slot];
                assert!(y - rotated.height / 2. >= bottom);
                assert!(
                    (layout.sizes[slot].width / layout.sizes[slot].height
                        - native.width / native.height)
                        .abs()
                        < 0.001
                );
                assert!(x - rotated.width / 2. >= 0.);
                assert!(x + rotated.width / 2. <= layout.width);
                bottom = y + rotated.height / 2.;
                for other in 0..slot {
                    assert_ne!(super::angle(slot), super::angle(other));
                }
            }
            assert!(bottom <= layout.height);
            assert!(layout.width < 220. && layout.height < 800.);
        }
        assert_eq!(
            super::collapsed(&[super::fitted(160, 90); 6]).centers.len(),
            3
        );
    }

    #[test]
    fn rotation_sheet_has_distinct_intermediate_frames_and_preserves_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("original.png");
        let preview = directory.path().join("sheet.png");
        image::RgbaImage::from_pixel(80, 120, image::Rgba([20, 80, 180, 255]))
            .save(&source)
            .unwrap();
        let bytes = std::fs::read(&source).unwrap();
        super::render(&source, &preview, 2, 80, 120).unwrap();
        assert_eq!(std::fs::read(source).unwrap(), bytes);
        let sheet = image::open(preview).unwrap().to_rgba8();
        assert_eq!(sheet.dimensions(), (1200, 1600));
        let cell = |index: u32| {
            image::imageops::crop_imm(&sheet, index % 5 * 240, index / 5 * 320, 240, 320).to_image()
        };
        let tilted = cell(0);
        let edge = tilted
            .pixels()
            .filter(|pixel| pixel[3] > 0 && pixel[3] < 255)
            .collect::<Vec<_>>();
        assert!(edge.len() > 100, "rotated edges need fractional coverage");
        assert!(
            edge.iter()
                .all(|pixel| pixel[0] == 20 && pixel[1] == 80 && pixel[2] == 180)
        );
        assert_ne!(cell(0), cell(12));
        assert_ne!(cell(12), cell(24));
        assert_eq!(super::frame(0.5), 12);
    }
}
