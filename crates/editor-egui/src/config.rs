//! Declarative host configuration for the native editor surface.
//!
//! Configuration controls supplied tools and interactions. It never filters
//! document content, changes Markdown spelling, or removes undo history.

use std::{collections::HashSet, error::Error, fmt, path::Path};

use glyphfolio_editor_core::{EditCommand, PresentationMode};
use serde::{Deserialize, Serialize};

/// The currently supported on-disk schema version.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorConfig {
    pub schema_version: u32,
    pub editor: EditorOptions,
    pub appearance: AppearanceConfig,
    pub document: DocumentConfig,
    pub toolbar: ToolbarConfig,
    pub features: Features,
    pub status: StatusConfig,
    pub diagnostics: DiagnosticsConfig,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            editor: EditorOptions::default(),
            appearance: AppearanceConfig::default(),
            document: DocumentConfig::default(),
            toolbar: ToolbarConfig::default(),
            features: Features::default(),
            status: StatusConfig::default(),
            diagnostics: DiagnosticsConfig::default(),
        }
    }
}

impl EditorConfig {
    /// GlyphFolio's editor appearance, with explicit experimental Rich opt-in.
    pub fn glyphfolio() -> Self {
        Self {
            editor: EditorOptions {
                default_presentation: Presentation::Rich,
                allow_experimental_rich: true,
            },
            ..Self::default()
        }
    }

    /// All existing tools, including native input controls and diagnostics.
    pub fn full() -> Self {
        Self {
            diagnostics: DiagnosticsConfig {
                visible: true,
                native_input_controls: true,
            },
            ..Self::glyphfolio()
        }
    }

    /// A compact toolbar; unlisted tools still work through their shortcuts/API.
    pub fn minimal() -> Self {
        Self {
            toolbar: ToolbarConfig {
                visible: true,
                groups: vec![
                    vec![ToolbarItem::Undo, ToolbarItem::Redo],
                    vec![ToolbarItem::Bold, ToolbarItem::Italic],
                    vec![ToolbarItem::Presentation],
                ],
            },
            ..Self::default()
        }
    }

    /// Parse and validate a configuration without writing any file or state.
    pub fn from_toml(source: &str) -> Result<Self, ConfigError> {
        let config: Self = toml_edit::de::from_str(source)
            .map_err(|error| ConfigError::Parse(error.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    /// Load only the explicit path supplied by the host. No search or creation.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path).map_err(|error| ConfigError::Read {
            path: path.to_string_lossy().into_owned(),
            reason: error.to_string(),
        })?;
        Self::from_toml(&source)
    }

    /// Export validated settings for a host-owned save service.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        self.validate()?;
        toml_edit::ser::to_string_pretty(self)
            .map_err(|error| ConfigError::Serialize(error.to_string()))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != SCHEMA_VERSION {
            return invalid("schema_version", "only version 1 is supported");
        }
        finite_range("document.font_size", self.document.font_size, 8.0, 72.0)?;
        finite_range("document.line_height", self.document.line_height, 1.0, 3.0)?;
        finite_range("document.max_width", self.document.max_width, 160.0, 8192.0)?;
        finite_range("document.padding", self.document.padding, 0.0, 256.0)?;
        finite_range(
            "document.inner_padding",
            self.document.inner_padding,
            0.0,
            128.0,
        )?;
        if self.document.inner_padding * 2.0 + 80.0 > self.document.max_width {
            return invalid(
                "document.inner_padding",
                "must leave at least 80 points of usable document width",
            );
        }
        match self.editor.default_presentation {
            Presentation::Source if !self.features.source => {
                return invalid(
                    "editor.default_presentation",
                    "Source is disabled in features",
                );
            }
            Presentation::Rich if !self.features.rich => {
                return invalid(
                    "editor.default_presentation",
                    "Rich is disabled in features",
                );
            }
            Presentation::Rich if !self.editor.allow_experimental_rich => {
                return invalid(
                    "editor.default_presentation",
                    "Rich requires editor.allow_experimental_rich = true",
                );
            }
            _ => {}
        }
        if !self.features.source && !self.features.rich {
            return invalid(
                "features",
                "at least one presentation must remain available",
            );
        }
        let mut seen = HashSet::new();
        for group in &self.toolbar.groups {
            if group.is_empty() {
                return invalid("toolbar.groups", "groups must contain at least one item");
            }
            for item in group {
                if !seen.insert(item) {
                    return invalid(
                        "toolbar.groups",
                        &format!("duplicate toolbar item {}", item.as_str()),
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorOptions {
    pub default_presentation: Presentation,
    pub allow_experimental_rich: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presentation {
    #[default]
    Source,
    Rich,
}

impl From<Presentation> for PresentationMode {
    fn from(value: Presentation) -> Self {
        match value {
            Presentation::Source => Self::Source,
            Presentation::Rich => Self::Rich,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppearanceConfig {
    pub theme: ThemeMode,
    pub frame: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DocumentConfig {
    /// Body font size in egui points.
    pub font_size: f32,
    /// Line-height multiplier, relative to body font size.
    pub line_height: f32,
    /// Maximum editor column width in points; narrow hosts still fit their UI.
    pub max_width: f32,
    /// Outer horizontal document gutters in points.
    pub padding: f32,
    /// Inset inside the editing surface in points.
    pub inner_padding: f32,
}

impl Default for DocumentConfig {
    fn default() -> Self {
        Self {
            font_size: 16.0,
            line_height: 1.6,
            max_width: 760.0,
            padding: 32.0,
            inner_padding: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolbarConfig {
    pub visible: bool,
    /// Ordered groups. Disabled tools are omitted; an empty outer list is valid.
    pub groups: Vec<Vec<ToolbarItem>>,
}

impl Default for ToolbarConfig {
    fn default() -> Self {
        Self {
            visible: true,
            groups: vec![
                vec![ToolbarItem::Undo, ToolbarItem::Redo],
                vec![ToolbarItem::Heading],
                vec![
                    ToolbarItem::Bold,
                    ToolbarItem::Italic,
                    ToolbarItem::Strikethrough,
                    ToolbarItem::Link,
                    ToolbarItem::InlineCode,
                ],
                vec![
                    ToolbarItem::BulletList,
                    ToolbarItem::OrderedList,
                    ToolbarItem::TaskList,
                    ToolbarItem::TaskChecked,
                    ToolbarItem::Table,
                    ToolbarItem::Blockquote,
                    ToolbarItem::CodeFence,
                ],
                vec![ToolbarItem::Outdent, ToolbarItem::Indent],
                vec![
                    ToolbarItem::Paste,
                    ToolbarItem::Presentation,
                    ToolbarItem::Save,
                ],
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolbarItem {
    Undo,
    Redo,
    Heading,
    Bold,
    Italic,
    Strikethrough,
    TaskList,
    TaskChecked,
    Table,
    Link,
    Paste,
    InlineCode,
    BulletList,
    OrderedList,
    Blockquote,
    CodeFence,
    Indent,
    Outdent,
    Presentation,
    Save,
}

impl ToolbarItem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Heading => "heading",
            Self::Bold => "bold",
            Self::Italic => "italic",
            Self::Strikethrough => "strikethrough",
            Self::TaskList => "task_list",
            Self::TaskChecked => "task_checked",
            Self::Table => "table",
            Self::Link => "link",
            Self::Paste => "paste",
            Self::InlineCode => "inline_code",
            Self::BulletList => "bullet_list",
            Self::OrderedList => "ordered_list",
            Self::Blockquote => "blockquote",
            Self::CodeFence => "code_fence",
            Self::Indent => "indent",
            Self::Outdent => "outdent",
            Self::Presentation => "presentation",
            Self::Save => "save",
        }
    }
}

/// Supplied tool/interaction availability, distinct from toolbar visibility.
/// Existing Markdown is still preserved, displayed, and editable as source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Features {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub task_lists: bool,
    pub tables: bool,
    pub inline_code: bool,
    pub headings: bool,
    pub lists: bool,
    pub blockquote: bool,
    pub code_fence: bool,
    pub indentation: bool,
    pub links: bool,
    pub history: bool,
    pub clipboard: bool,
    pub formatted_paste: bool,
    pub images: bool,
    pub attachments: bool,
    pub source: bool,
    pub rich: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            bold: true,
            italic: true,
            strikethrough: true,
            task_lists: true,
            tables: true,
            inline_code: true,
            headings: true,
            lists: true,
            blockquote: true,
            code_fence: true,
            indentation: true,
            links: true,
            history: true,
            clipboard: true,
            formatted_paste: true,
            images: true,
            attachments: true,
            source: true,
            rich: true,
        }
    }
}

impl Features {
    pub fn allows_item(&self, item: ToolbarItem) -> bool {
        match item {
            ToolbarItem::Undo | ToolbarItem::Redo => self.history,
            ToolbarItem::Heading => self.headings,
            ToolbarItem::Bold => self.bold,
            ToolbarItem::Italic => self.italic,
            ToolbarItem::Table => self.tables,
            ToolbarItem::Link => self.links,
            ToolbarItem::Paste => self.clipboard,
            ToolbarItem::Strikethrough => self.strikethrough,
            ToolbarItem::TaskList | ToolbarItem::TaskChecked => self.task_lists,
            ToolbarItem::InlineCode => self.inline_code,
            ToolbarItem::BulletList | ToolbarItem::OrderedList => self.lists,
            ToolbarItem::Blockquote => self.blockquote,
            ToolbarItem::CodeFence => self.code_fence,
            ToolbarItem::Indent | ToolbarItem::Outdent => self.indentation,
            ToolbarItem::Presentation => self.source || self.rich,
            ToolbarItem::Save => true,
        }
    }

    pub fn allows_command(&self, command: EditCommand) -> bool {
        match command {
            EditCommand::ToggleStrong => self.bold,
            EditCommand::ToggleEmphasis => self.italic,
            EditCommand::Table(_) => self.tables,
            EditCommand::ToggleStrikethrough => self.strikethrough,
            EditCommand::ToggleTaskList | EditCommand::ToggleTaskChecked => self.task_lists,
            EditCommand::ToggleInlineCode => self.inline_code,
            EditCommand::SetHeading(_) => self.headings,
            EditCommand::ToggleUnorderedList | EditCommand::ToggleOrderedList => self.lists,
            EditCommand::ToggleBlockQuote => self.blockquote,
            EditCommand::ToggleCodeFence => self.code_fence,
            EditCommand::Indent | EditCommand::Outdent => self.indentation,
            EditCommand::Enter | EditCommand::Backspace => true,
        }
    }

    pub fn allows_presentation(&self, presentation: PresentationMode) -> bool {
        match presentation {
            PresentationMode::Source => self.source,
            PresentationMode::Rich => self.rich,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StatusConfig {
    pub visible: bool,
    pub word_count: bool,
}

impl Default for StatusConfig {
    fn default() -> Self {
        Self {
            visible: true,
            word_count: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiagnosticsConfig {
    pub visible: bool,
    pub native_input_controls: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    Read { path: String, reason: String },
    Parse(String),
    Invalid { field: String, reason: String },
    Serialize(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, reason } => {
                write!(
                    formatter,
                    "Cannot read editor configuration {path}: {reason}"
                )
            }
            Self::Parse(reason) => write!(formatter, "Invalid editor TOML: {reason}"),
            Self::Invalid { field, reason } => {
                write!(formatter, "Invalid editor setting {field}: {reason}")
            }
            Self::Serialize(reason) => write!(formatter, "Cannot export editor TOML: {reason}"),
        }
    }
}

impl Error for ConfigError {}

fn invalid<T>(field: &str, reason: &str) -> Result<T, ConfigError> {
    Err(ConfigError::Invalid {
        field: field.to_owned(),
        reason: reason.to_owned(),
    })
}

fn finite_range(field: &str, value: f32, minimum: f32, maximum: f32) -> Result<(), ConfigError> {
    if !value.is_finite() || !(minimum..=maximum).contains(&value) {
        return invalid(
            field,
            &format!("must be a finite number from {minimum} to {maximum}"),
        );
    }
    Ok(())
}
