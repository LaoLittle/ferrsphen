use anyhow::{Context, Result};
use image::{AnimationDecoder, ImageReader};
use std::{
    io::BufReader,
    path::{Path, PathBuf},
};

static CONVERSIONS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
const EDGE: u32 = 96;

fn key_name(key: &str) -> String {
    use md5::{Digest, Md5};
    format!("asset-v1-{}", hex::encode(Md5::digest(key.as_bytes())))
}
pub(crate) async fn cached(root: &Path, key: &str) -> Result<Option<PathBuf>> {
    let directory = root.join("emojis");
    let name = key_name(key);
    for extension in ["svg", "json", "gif", "png", "jpg", "webp"] {
        let path = directory.join(format!("{name}.{extension}"));
        if tokio::fs::try_exists(&path).await? {
            return Ok(Some(path));
        }
    }
    Ok(None)
}
pub(crate) async fn remember_key(root: &Path, key: &str, prepared: &Path) -> Result<PathBuf> {
    let extension = prepared
        .extension()
        .and_then(|value| value.to_str())
        .context("emoji extension unavailable")?;
    let directory = root.join("emojis");
    tokio::fs::create_dir_all(&directory).await?;
    let path = directory.join(format!("{}.{extension}", key_name(key)));
    if path.is_file() {
        return Ok(path);
    }
    if tokio::fs::hard_link(prepared, &path).await.is_ok() {
        return Ok(path);
    }
    let temporary = directory.join(format!(".{:016x}.part", rand::random::<u64>()));
    tokio::fs::copy(prepared, &temporary).await?;
    if let Err(error) = tokio::fs::rename(&temporary, &path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        if !path.is_file() {
            return Err(error.into());
        }
    }
    Ok(path)
}

pub(crate) async fn remember_vector(
    root: &Path,
    key: &str,
    source: &Path,
    extension: &str,
) -> Result<PathBuf> {
    let bytes = tokio::fs::read(source).await?;
    if extension == "json" {
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            value["layers"].is_array()
                && value["w"].as_u64().is_some()
                && value["h"].as_u64().is_some(),
            "invalid Lottie asset"
        );
    } else {
        anyhow::ensure!(
            std::str::from_utf8(&bytes)?.contains("<svg"),
            "invalid SVG asset"
        );
    }
    let directory = root.join("emojis");
    tokio::fs::create_dir_all(&directory).await?;
    let path = directory.join(format!("{}.{extension}", key_name(key)));
    let temporary = directory.join(format!(".{:016x}.part", rand::random::<u64>()));
    tokio::fs::write(&temporary, bytes).await?;
    if let Err(error) = tokio::fs::rename(&temporary, &path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        if !path.is_file() {
            return Err(error.into());
        }
    }
    Ok(path)
}

pub(crate) async fn prepare(source: &Path, root: &Path) -> Result<PathBuf> {
    let _permit = CONVERSIONS.acquire().await?;
    let source = source.to_owned();
    let root = root.to_owned();
    tokio::task::spawn_blocking(move || prepare_sync(&source, &root)).await?
}
fn prepare_sync(source: &Path, root: &Path) -> Result<PathBuf> {
    let directory = root.join("emojis");
    let name = source.file_name().context("emoji filename unavailable")?;
    let path = directory.join(format!("v1-{}", name.to_string_lossy()));
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(&directory)?;
    let temporary = directory.join(format!(".{:016x}.part", rand::random::<u64>()));
    let result = (|| -> Result<()> {
        let reader = ImageReader::open(source)?.with_guessed_format()?;
        if reader.format() == Some(image::ImageFormat::Gif) {
            let decoder =
                image::codecs::gif::GifDecoder::new(BufReader::new(std::fs::File::open(source)?))?;
            let output = std::fs::File::create(&temporary)?;
            let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(output, 10);
            encoder.set_repeat(image::codecs::gif::Repeat::Infinite)?;
            for frame in decoder.into_frames() {
                let frame = frame?;
                let delay = frame.delay();
                let buffer = image::DynamicImage::ImageRgba8(frame.into_buffer())
                    .thumbnail(EDGE, EDGE)
                    .into_rgba8();
                encoder.encode_frame(image::Frame::from_parts(buffer, 0, 0, delay))?;
            }
        } else {
            let format = reader.format().context("emoji image format unavailable")?;
            reader
                .decode()?
                .thumbnail(EDGE, EDGE)
                .save_with_format(&temporary, format)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        if !path.is_file() {
            return Err(error.into());
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn prepared_animation_keeps_frames_timing_and_original_bytes() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("original.gif");
        let mut encoder =
            image::codecs::gif::GifEncoder::new(std::fs::File::create(&source).unwrap());
        for (red, delay) in [(20, 80), (200, 120)] {
            encoder
                .encode_frame(image::Frame::from_parts(
                    image::RgbaImage::from_pixel(512, 256, image::Rgba([red, 80, 180, 255])),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(delay, 1),
                ))
                .unwrap();
        }
        drop(encoder);
        let original = std::fs::read(&source).unwrap();
        let prepared = prepare(&source, root.path()).await.unwrap();
        let frames = image::codecs::gif::GifDecoder::new(BufReader::new(
            std::fs::File::open(&prepared).unwrap(),
        ))
        .unwrap()
        .into_frames()
        .collect_frames()
        .unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].buffer().dimensions(), (96, 48));
        assert_eq!(frames[0].delay().numer_denom_ms(), (80, 1));
        assert_eq!(frames[1].delay().numer_denom_ms(), (120, 1));
        assert_eq!(std::fs::read(&source).unwrap(), original);
        std::fs::remove_file(&source).unwrap();
        assert_eq!(prepare(&source, root.path()).await.unwrap(), prepared);
    }
}
