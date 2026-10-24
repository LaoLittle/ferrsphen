use anyhow::{Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui_kit::{ClipboardEntry, ClipboardItem, Image, ImageFormat};
use serde::{Deserialize, Serialize};
use std::{
    io::Cursor,
    path::{Path, PathBuf},
};

const FORMAT: &str = "ferrsphen.richtext.v1";
const MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Part {
    Text { text: String },
    Image { data: String },
    Face { face: crate::expressions::Face },
}
#[derive(Serialize, Deserialize)]
struct Fragment {
    format: String,
    parts: Vec<Part>,
}

pub fn text(text: impl Into<String>) -> Part {
    Part::Text { text: text.into() }
}
pub fn image(data: &[u8]) -> Result<Part> {
    ensure!(data.len() <= MAX_BYTES, "图片过大");
    image_format(data)?;
    Ok(Part::Image {
        data: STANDARD.encode(data),
    })
}
pub fn image_file(path: &Path) -> Result<Part> {
    ensure!(
        std::fs::metadata(path)?.len() <= MAX_BYTES as u64,
        "图片过大"
    );
    image(&std::fs::read(path)?)
}
fn image_format(data: &[u8]) -> Result<ImageFormat> {
    Ok(match image::guess_format(data)? {
        image::ImageFormat::Png => ImageFormat::Png,
        image::ImageFormat::Jpeg => ImageFormat::Jpeg,
        image::ImageFormat::WebP => ImageFormat::Webp,
        image::ImageFormat::Gif => ImageFormat::Gif,
        image::ImageFormat::Bmp => ImageFormat::Bmp,
        image::ImageFormat::Tiff => ImageFormat::Tiff,
        _ => bail!("不支持的图片格式"),
    })
}
pub fn clipboard(parts: Vec<Part>) -> Result<ClipboardItem> {
    ensure!(!parts.is_empty(), "内容为空");
    if let [Part::Image { data }] = parts.as_slice() {
        let data = STANDARD.decode(data)?;
        return Ok(ClipboardItem::new_image(&Image::from_bytes(
            image_format(&data)?,
            data,
        )));
    }
    let plain = parts
        .iter()
        .map(|part| match part {
            Part::Text { text } => text.clone(),
            Part::Image { .. } => "[图片]".into(),
            Part::Face { face } => face.text(),
        })
        .collect::<String>();
    if parts.iter().all(|part| matches!(part, Part::Text { .. })) {
        return Ok(ClipboardItem::new_string(plain));
    }
    let metadata = serde_json::to_string(&Fragment {
        format: FORMAT.into(),
        parts,
    })?;
    Ok(ClipboardItem::new_string_with_metadata(plain, metadata))
}
/// None leaves ordinary text editing to the input's native clipboard action.
pub fn rich_parts(item: &ClipboardItem) -> Result<Option<Vec<Part>>> {
    for entry in item.entries() {
        if let ClipboardEntry::String(string) = entry
            && let Some(metadata) = &string.metadata
            && metadata.len() <= MAX_BYTES * 8
            && let Ok(fragment) = serde_json::from_str::<Fragment>(metadata)
            && fragment.format == FORMAT
        {
            ensure!(
                !fragment.parts.is_empty() && fragment.parts.len() <= 1024,
                "剪贴板内容过大"
            );
            let mut total = 0;
            for part in &fragment.parts {
                match part {
                    Part::Text { text } => total += text.len(),
                    Part::Face { face } => {
                        ensure!(face.id <= 65535 && face.name.len() <= 256, "表情无效");
                        total += face.name.len();
                    }
                    Part::Image { data } => {
                        ensure!(data.len() <= MAX_BYTES.div_ceil(3) * 4, "图片过大");
                        let bytes = STANDARD.decode(data)?;
                        image_format(&bytes)?;
                        total += bytes.len();
                    }
                }
                ensure!(total <= MAX_BYTES * 4, "剪贴板内容过大");
            }
            return Ok(Some(fragment.parts));
        }
    }
    if !item
        .entries()
        .iter()
        .any(|entry| matches!(entry, ClipboardEntry::Image(_)))
    {
        return Ok(None);
    }
    item.entries()
        .iter()
        .filter_map(|entry| match entry {
            ClipboardEntry::String(string) => Some(Ok(text(string.text.clone()))),
            ClipboardEntry::Image(value) => Some(image(&value.bytes)),
            _ => None,
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}
pub fn materialize(part: &Part, root: &Path) -> Result<Option<PathBuf>> {
    let Part::Image { data } = part else {
        return Ok(None);
    };
    let data = STANDARD.decode(data)?;
    ensure!(data.len() <= MAX_BYTES, "图片过大");
    let format = image_format(&data)?;
    let (extension, data) = match format {
        ImageFormat::Png => ("png", data),
        ImageFormat::Jpeg => ("jpg", data),
        ImageFormat::Webp => ("webp", data),
        ImageFormat::Gif => ("gif", data),
        _ => {
            let mut output = Cursor::new(Vec::new());
            image::load_from_memory(&data)?.write_to(&mut output, image::ImageFormat::Png)?;
            ("png", output.into_inner())
        }
    };
    let path = root.join(format!("{:032x}.{extension}", rand::random::<u128>()));
    std::fs::write(&path, data)?;
    Ok(Some(path))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn png() -> Vec<u8> {
        let mut data = Cursor::new(Vec::new());
        image::RgbaImage::new(2, 2)
            .write_to(&mut data, image::ImageFormat::Png)
            .unwrap();
        data.into_inner()
    }
    #[test]
    fn mixed_fragment_survives_the_native_string_metadata_roundtrip() {
        let bytes = png();
        let item = clipboard(vec![text("前"), image(&bytes).unwrap(), text("后")]).unwrap();
        assert_eq!(item.text().as_deref(), Some("前[图片]后"));
        let parts = rich_parts(&item).unwrap().unwrap();
        assert!(
            matches!(parts.as_slice(),[Part::Text {text:before},Part::Image{..},Part::Text{text:after}] if before == "前" && after == "后")
        );
        let root = tempfile::tempdir().unwrap();
        let path = materialize(&parts[1], root.path()).unwrap().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn native_bitmap_and_tiff_images_are_converted_to_png_for_sending() {
        let root = tempfile::tempdir().unwrap();
        for format in [image::ImageFormat::Bmp, image::ImageFormat::Tiff] {
            let mut bytes = Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(4, 3, image::Rgba([40, 80, 120, 255]))
                .write_to(&mut bytes, format)
                .unwrap();
            let part = image(&bytes.into_inner()).unwrap();
            let path = materialize(&part, root.path()).unwrap().unwrap();
            assert_eq!(path.extension().unwrap(), "png");
            assert_eq!(image::image_dimensions(path).unwrap(), (4, 3));
        }
    }

    #[test]
    fn one_picture_uses_the_native_image_clipboard_and_invalid_data_is_rejected() {
        let item = clipboard(vec![image(&png()).unwrap()]).unwrap();
        assert!(matches!(item.entries(), [ClipboardEntry::Image(_)]));
        assert!(matches!(
            rich_parts(&item).unwrap().unwrap().as_slice(),
            [Part::Image { .. }]
        ));
        assert!(
            rich_parts(&ClipboardItem::new_string("text".into()))
                .unwrap()
                .is_none()
        );
        let bad = ClipboardItem::new_string_with_metadata(
            "[图片]".into(),
            r#"{"format":"ferrsphen.richtext.v1","parts":[{"kind":"image","data":"!!!"}]}"#.into(),
        );
        assert!(rich_parts(&bad).is_err());
    }
}
