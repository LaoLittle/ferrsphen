use anyhow::Result;
use atri_qq::media::MediaReference;
use md5::{Digest, Md5};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{Mutex, OnceCell, OwnedMutexGuard};

const EXTENSIONS: [&str; 10] = [
    "png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "tiff", "ico", "qoi",
];

pub(crate) struct ImageCache {
    directory: PathBuf,
    indexed: OnceCell<()>,
    pending: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}
impl ImageCache {
    pub fn new(root: &Path) -> Self {
        Self {
            directory: root.join("images"),
            indexed: OnceCell::new(),
            pending: Mutex::new(HashMap::new()),
        }
    }
    pub async fn lock(&self, reference: &MediaReference, key: &str) -> OwnedMutexGuard<()> {
        let identity = hashes(reference).next().unwrap_or_else(|| key.to_owned());
        let lock = {
            let mut pending = self.pending.lock().await;
            pending.retain(|_, lock| Arc::strong_count(lock) > 1);
            pending.entry(identity).or_default().clone()
        };
        lock.lock_owned().await
    }
    pub async fn find(&self, reference: &MediaReference) -> Result<Option<PathBuf>> {
        if hashes(reference).next().is_none() {
            return Ok(None);
        }
        self.indexed
            .get_or_try_init(|| async {
                let directory = self.directory.clone();
                tokio::task::spawn_blocking(move || index_existing(&directory)).await??;
                Ok::<_, anyhow::Error>(())
            })
            .await?;
        for hash in hashes(reference) {
            for extension in EXTENSIONS {
                let path = self.directory.join(format!("{hash}.{extension}"));
                if tokio::fs::try_exists(&path).await? {
                    return Ok(Some(path));
                }
            }
        }
        Ok(None)
    }
    pub async fn remember(&self, source: &Path) -> Result<PathBuf> {
        let directory = self.directory.clone();
        let source = source.to_owned();
        tokio::task::spawn_blocking(move || index_file(&directory, &source)).await?
    }
}
fn hashes(reference: &MediaReference) -> impl Iterator<Item = String> + '_ {
    (reference.md5.len() == 16)
        .then(|| format!("md5-{}", hex::encode(&reference.md5)))
        .into_iter()
        .chain(
            (reference.sha1.len() == 20).then(|| format!("sha1-{}", hex::encode(&reference.sha1))),
        )
}
fn index_existing(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let mut entries = std::fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    if let Some(parent) = directory.parent() {
        entries.extend(std::fs::read_dir(parent)?.collect::<std::io::Result<Vec<_>>>()?);
    }
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("md5-") || name.starts_with("sha1-") || name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_file()
            && let Err(error) = index_file(directory, &path)
        {
            tracing::debug!(%error, path = %path.display(), "skip unreadable image cache entry");
        }
    }
    Ok(())
}
fn index_file(directory: &Path, source: &Path) -> Result<PathBuf> {
    let bytes = std::fs::read(source)?;
    let format = image::guess_format(&bytes)?;
    let extension = format.extensions_str()[0];
    let md5 = hex::encode(Md5::digest(&bytes));
    let sha1 = sha1_smol::Sha1::from(&bytes).digest().to_string();
    std::fs::create_dir_all(directory)?;
    let original = directory.join(format!("md5-{md5}.{extension}"));
    for target in [
        &original,
        &directory.join(format!("sha1-{sha1}.{extension}")),
    ] {
        if target.is_file() {
            continue;
        }
        let link_source = if source.parent() == Some(directory) {
            Some(source)
        } else if original.is_file() {
            Some(original.as_path())
        } else {
            None
        };
        if let Some(source) = link_source
            && std::fs::hard_link(source, target).is_ok()
        {
            continue;
        }
        // A temporary file keeps fallback copies invisible until complete.
        let temporary = directory.join(format!(".{:016x}.part", rand::random::<u64>()));
        std::fs::write(&temporary, &bytes)?;
        if let Err(error) = std::fs::rename(&temporary, target) {
            let _ = std::fs::remove_file(&temporary);
            if !target.is_file() {
                return Err(error.into());
            }
        }
    }
    Ok(original)
}

#[cfg(test)]
mod tests {
    use super::*;
    use atri_qq::media::{MediaConversation, MediaKind};

    fn picture(format: image::ImageFormat) -> Vec<u8> {
        let mut output = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            16,
            12,
            image::Rgba([20, 80, 180, 255]),
        ))
        .write_to(&mut output, format)
        .unwrap();
        output.into_inner()
    }
    fn reference(bytes: &[u8]) -> MediaReference {
        let mut reference = MediaReference::new(
            MediaKind::Image,
            MediaConversation::Friend { uid: "peer".into() },
        );
        reference.md5 = Md5::digest(bytes).to_vec();
        reference.sha1 = sha1_smol::Sha1::from(bytes).digest().bytes().to_vec();
        reference
    }
    #[tokio::test]
    async fn old_cache_files_are_indexed_by_content_without_reencoding() {
        for format in [image::ImageFormat::Png, image::ImageFormat::Gif] {
            for legacy_root in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let bytes = picture(format);
                let directory = root.path().join("images");
                std::fs::create_dir_all(&directory).unwrap();
                let source = if legacy_root {
                    root.path().join("legacy-no-extension")
                } else {
                    directory.join("old-message-key.gif")
                };
                std::fs::write(&source, &bytes).unwrap();
                let reference = reference(&bytes);
                let cache = ImageCache::new(root.path());
                let path = cache.find(&reference).await.unwrap().unwrap();
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                assert_eq!(path.extension().unwrap(), format.extensions_str()[0]);
                assert!(source.is_file());
                let mut sha1_only = reference.clone();
                sha1_only.md5.clear();
                assert_eq!(
                    std::fs::read(cache.find(&sha1_only).await.unwrap().unwrap()).unwrap(),
                    bytes
                );
                assert_eq!(
                    ImageCache::new(root.path()).find(&reference).await.unwrap(),
                    Some(path)
                );
            }
        }
    }
    #[tokio::test]
    async fn outgoing_original_is_immutable_and_can_be_reused_by_its_hash() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("outgoing.png");
        let bytes = picture(image::ImageFormat::Png);
        std::fs::write(&source, &bytes).unwrap();
        let cache = ImageCache::new(&root.path().join("cache"));
        let path = cache.remember(&source).await.unwrap();
        std::fs::write(&source, b"changed externally").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(cache.find(&reference(&bytes)).await.unwrap(), Some(path));
    }
    #[tokio::test]
    async fn unknown_and_malformed_hashes_do_not_match_other_images() {
        let root = tempfile::tempdir().unwrap();
        let cache = ImageCache::new(root.path());
        let mut reference = reference(b"not cached");
        assert!(cache.find(&reference).await.unwrap().is_none());
        reference.md5 = vec![0xff; 15];
        reference.sha1 = vec![0xff; 19];
        assert!(cache.find(&reference).await.unwrap().is_none());
    }
}
