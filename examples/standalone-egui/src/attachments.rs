//! Example host policy: local references within an explicit root; PNG previews.
use glyphfolio_editor_egui::assets::{
    AssetImage, AssetImport, AssetService, ImportedAsset, MAX_IMAGE_PIXELS,
};
use std::{
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};
pub struct LocalAttachments {
    root: PathBuf,
}
impl LocalAttachments {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        Ok(Self {
            root: root.canonicalize().map_err(|e| e.to_string())?,
        })
    }
}
fn decode(bytes: &[u8]) -> Result<AssetImage, String> {
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("Image file exceeds 16 MiB".into());
    }
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let (width, height) = reader.into_dimensions().map_err(|e| e.to_string())?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS as u64 {
        return Err("Image exceeds pixel limit".into());
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| e.to_string())?.into_rgba8();
    Ok(AssetImage {
        size: [width as usize, height as usize],
        rgba: image.into_raw(),
    })
}
fn read(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("Attachment exceeds 16 MiB".into());
    }
    Ok(bytes)
}
impl AssetService for LocalAttachments {
    fn resolve_image(&mut self, destination: &str) -> Result<Option<AssetImage>, String> {
        let path = self.root.join(destination);
        let Ok(path) = path.canonicalize() else {
            return Ok(None);
        };
        if !path.starts_with(&self.root) {
            return Err("Attachment is outside the configured root".into());
        }
        let bytes = read(&path)?;
        Ok(decode(&bytes).ok())
    }
    fn import(&mut self, input: AssetImport) -> Result<ImportedAsset, String> {
        let (name, bytes) = match input {
            AssetImport::Image(image) => {
                image.validate()?;
                let buffer = image::RgbaImage::from_raw(
                    image.size[0] as u32,
                    image.size[1] as u32,
                    image.rgba,
                )
                .ok_or("Invalid clipboard image")?;
                let mut output = Cursor::new(Vec::new());
                buffer
                    .write_to(&mut output, image::ImageFormat::Png)
                    .map_err(|error| error.to_string())?;
                ("clipboard.png".into(), output.into_inner())
            }
            AssetImport::File(path) => (
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                read(&path)?,
            ),
            AssetImport::Bytes { name, bytes } => (name, bytes.to_vec()),
        };
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("Attachment exceeds 16 MiB".into());
        }
        let safe: String = name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                    ch
                } else {
                    '_'
                }
            })
            .take(100)
            .collect();
        let name = if safe.is_empty() { "attachment" } else { &safe };
        let folder = self.root.join("assets");
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        if !folder
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(&self.root)
        {
            return Err("Attachment folder is outside the configured root".into());
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let destination = format!("assets/{stamp}-{name}");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.root.join(&destination))
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        Ok(ImportedAsset {
            destination,
            label: name.into(),
            image: decode(&bytes).is_ok(),
        })
    }
}
