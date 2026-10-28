//! Explicit host-owned assets. The editor never fetches URLs or reads paths itself.
use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use glyphfolio_editor_core::{Editor, TransactionOrigin};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

pub const MAX_IMAGE_PIXELS: usize = 4_194_304;
const CACHE_BYTES: usize = 32 * 1024 * 1024;
/// Already decoded RGBA pixels. Hosts must bound decoder allocations as well.
pub struct AssetImage {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}
impl AssetImage {
    pub fn validate(&self) -> Result<(), String> {
        let pixels = self.size[0]
            .checked_mul(self.size[1])
            .filter(|pixels| *pixels > 0 && *pixels <= MAX_IMAGE_PIXELS)
            .ok_or("Image exceeds the pixel limit")?;
        if self.rgba.len() != pixels * 4 {
            return Err("Invalid RGBA image length".into());
        }
        Ok(())
    }
}
pub enum AssetImport {
    Image(AssetImage),
    File(PathBuf),
    Bytes { name: String, bytes: Arc<[u8]> },
}
pub struct ImportedAsset {
    pub destination: String,
    pub label: String,
    pub image: bool,
}
/// Services own filesystem/network policy, storage and asynchronous prefetching.
/// `resolve_image` should return cached data or None rather than block the UI.
pub trait AssetService {
    fn resolve_image(&mut self, destination: &str) -> Result<Option<AssetImage>, String>;
    fn import(&mut self, input: AssetImport) -> Result<ImportedAsset, String>;
}
#[derive(Default)]
pub(crate) struct AssetCache {
    pub service: Option<Box<dyn AssetService>>,
    pub images: HashMap<String, AssetImage>,
    textures: HashMap<String, TextureHandle>,
    revision: Option<(
        glyphfolio_editor_core::DocumentId,
        glyphfolio_editor_core::Revision,
    )>,
}
impl AssetCache {
    pub fn refresh(&mut self, editor: &Editor) -> HashMap<String, egui::Vec2> {
        let snapshot = editor.snapshot();
        let key = (snapshot.document_id(), snapshot.revision());
        if self.revision != Some(key) {
            self.revision = Some(key);
            let references = glyphfolio_editor_markdown::image_references(&snapshot.text());
            self.images.retain(|destination, _| {
                references
                    .iter()
                    .any(|reference| &reference.destination == destination)
            });
            self.textures
                .retain(|destination, _| self.images.contains_key(destination));
            let mut bytes: usize = self.images.values().map(|image| image.rgba.len()).sum();
            if let Some(service) = &mut self.service {
                for reference in references.iter().take(64) {
                    if self.images.contains_key(&reference.destination) {
                        continue;
                    }
                    if let Ok(Some(image)) = service.resolve_image(&reference.destination)
                        && image.validate().is_ok()
                        && bytes + image.rgba.len() <= CACHE_BYTES
                    {
                        bytes += image.rgba.len();
                        self.images.insert(reference.destination.clone(), image);
                    }
                }
            }
        }
        self.images
            .iter()
            .map(|(destination, image)| {
                (
                    destination.clone(),
                    egui::vec2(image.size[0] as f32, image.size[1] as f32),
                )
            })
            .collect()
    }
    pub fn texture(&mut self, ctx: &Context, destination: &str) -> Option<egui::TextureId> {
        if !self.textures.contains_key(destination) {
            let image = self.images.get(destination)?;
            let texture = ctx.load_texture(
                format!("editor-asset-{destination}"),
                ColorImage::from_rgba_unmultiplied(image.size, &image.rgba),
                TextureOptions::LINEAR,
            );
            self.textures.insert(destination.into(), texture);
        }
        self.textures.get(destination).map(TextureHandle::id)
    }
    pub fn import(&mut self, editor: &mut Editor, input: AssetImport) -> Result<(), String> {
        self.import_many(editor, vec![input])
    }
    pub fn import_many(
        &mut self,
        editor: &mut Editor,
        inputs: Vec<AssetImport>,
    ) -> Result<(), String> {
        if inputs.is_empty() {
            return Ok(());
        }
        if inputs.len() > 64 {
            return Err("Paste at most 64 attachments at once".into());
        }
        let mut sources = Vec::new();
        let service = self
            .service
            .as_mut()
            .ok_or("No host attachment service is configured")?;
        for input in inputs {
            let imported = service.import(input)?;
            if !glyphfolio_editor_markdown::safe_link(&imported.destination) {
                return Err("Attachment service returned an unsafe reference".into());
            }
            let label = imported
                .label
                .replace('\\', "\\\\")
                .replace('[', "\\[")
                .replace(']', "\\]")
                .replace(['\r', '\n'], " ");
            let destination = imported.destination.replace('<', "%3C").replace('>', "%3E");
            let source = format!(
                "{}[{label}](<{destination}>)",
                if imported.image { "!" } else { "" }
            );
            sources.push(source);
        }
        editor
            .insert_text(&sources.join("\n"), TransactionOrigin::Paste)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}
