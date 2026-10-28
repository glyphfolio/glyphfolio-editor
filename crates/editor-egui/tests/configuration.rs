//! Real configuration contracts, independent of any particular toolbar paint.

use glyphfolio_editor_core::{EditCommand, HeadingLevel, PresentationMode};
use glyphfolio_editor_egui::config::{
    ConfigError, EditorConfig, Features, Presentation, ThemeMode, ToolbarItem,
};

#[test]
fn shipped_profiles_match_the_public_typed_presets() {
    assert_eq!(
        EditorConfig::from_toml(include_str!("../../../editor.toml")).unwrap(),
        EditorConfig::glyphfolio()
    );
    assert_eq!(
        EditorConfig::from_toml(include_str!("../../../config/minimal.toml")).unwrap(),
        EditorConfig::minimal()
    );
    assert_eq!(
        EditorConfig::from_toml(include_str!("../../../config/full.toml")).unwrap(),
        EditorConfig::full()
    );
    for config in [
        EditorConfig::default(),
        EditorConfig::glyphfolio(),
        EditorConfig::minimal(),
        EditorConfig::full(),
    ] {
        assert_eq!(
            EditorConfig::from_toml(&config.to_toml().unwrap()).unwrap(),
            config
        );
    }
}

#[test]
fn sparse_settings_keep_safe_defaults_and_preserve_explicit_toolbar_order() {
    let config = EditorConfig::from_toml(
        r#"
        [toolbar]
        groups = [["presentation"], ["italic", "bold"], ["save"]]
        [appearance]
        theme = "dark"
        "#,
    )
    .unwrap();
    assert_eq!(config.editor.default_presentation, Presentation::Source);
    assert!(!config.editor.allow_experimental_rich);
    assert_eq!(config.appearance.theme, ThemeMode::Dark);
    assert_eq!(config.toolbar.groups[0], vec![ToolbarItem::Presentation]);
    assert_eq!(
        config.toolbar.groups[1],
        vec![ToolbarItem::Italic, ToolbarItem::Bold]
    );
    assert_eq!(config.document.font_size, 16.0);
    assert_eq!(config.features, Features::default());
}

#[test]
fn toolbar_visibility_is_separate_from_command_availability() {
    let mut config = EditorConfig::minimal();
    config.toolbar.visible = false;
    assert!(config.features.allows_command(EditCommand::ToggleStrong));
    assert!(config.features.allows_command(EditCommand::ToggleCodeFence));
    assert!(config.features.clipboard);
    config.features.bold = false;
    assert!(!config.features.allows_item(ToolbarItem::Bold));
    assert!(!config.features.allows_command(EditCommand::ToggleStrong));
    assert!(config.features.allows_command(EditCommand::ToggleEmphasis));
    assert!(config.validate().is_ok());
}

#[test]
fn feature_policy_covers_structural_commands_without_disabling_base_editing() {
    let features = Features {
        bold: false,
        italic: false,
        inline_code: false,
        headings: false,
        lists: false,
        blockquote: false,
        code_fence: false,
        indentation: false,
        history: false,
        clipboard: false,
        links: false,
        ..Features::default()
    };
    for command in [
        EditCommand::ToggleStrong,
        EditCommand::ToggleEmphasis,
        EditCommand::ToggleInlineCode,
        EditCommand::SetHeading(Some(HeadingLevel::One)),
        EditCommand::SetHeading(None),
        EditCommand::ToggleUnorderedList,
        EditCommand::ToggleOrderedList,
        EditCommand::ToggleBlockQuote,
        EditCommand::ToggleCodeFence,
        EditCommand::Indent,
        EditCommand::Outdent,
    ] {
        assert!(!features.allows_command(command), "{command:?}");
    }
    assert!(!features.allows_item(ToolbarItem::Undo));
    assert!(!features.allows_item(ToolbarItem::Redo));
    assert!(features.allows_command(EditCommand::Enter));
    assert!(features.allows_command(EditCommand::Backspace));
    assert!(features.allows_presentation(PresentationMode::Source));
}

#[test]
fn unknown_keys_enums_versions_and_conflicting_modes_are_errors() {
    for (source, expected) in [
        ("schema_version = 2", "schema_version"),
        ("[toolbar]\nvisble = false", "visble"),
        ("[document]\nfont_size = \"large\"", "invalid type"),
        ("[appearance]\ntheme = \"ultraviolet\"", "ultraviolet"),
        ("[toolbar]\ngroups = [[\"not_a_tool\"]]", "not_a_tool"),
        (
            "[editor]\ndefault_presentation = \"rich\"",
            "allow_experimental_rich",
        ),
        ("[features]\nsource = false", "Source is disabled"),
        (
            "[editor]\ndefault_presentation = \"rich\"\nallow_experimental_rich = true\n[features]\nrich = false",
            "Rich is disabled",
        ),
        (
            "[toolbar]\ngroups = [[\"bold\"], [\"bold\"]]",
            "duplicate toolbar item bold",
        ),
        ("[toolbar]\ngroups = [[]]", "at least one item"),
    ] {
        let error = EditorConfig::from_toml(source).unwrap_err().to_string();
        assert!(error.contains(expected), "{source}: {error}");
    }
}

#[test]
fn geometry_rejects_nonfinite_and_out_of_range_values_before_application() {
    for source in [
        "[document]\nfont_size = nan",
        "[document]\nfont_size = inf",
        "[document]\nfont_size = 0",
        "[document]\nline_height = 0.5",
        "[document]\nmax_width = 0",
        "[document]\npadding = -1",
        "[document]\ninner_padding = -1",
        "[document]\nmax_width = 160\ninner_padding = 128",
    ] {
        assert!(
            matches!(
                EditorConfig::from_toml(source),
                Err(ConfigError::Invalid { .. })
            ),
            "{source}"
        );
    }
    let mut config = EditorConfig::default();
    config.document.font_size = f32::NAN;
    assert!(config.to_toml().is_err());
}

#[test]
fn explicit_loading_never_creates_missing_files_or_changes_existing_data() {
    let missing = std::env::temp_dir().join(format!(
        "glyphfolio-config-missing-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    assert!(!missing.exists());
    assert!(matches!(
        EditorConfig::load(&missing),
        Err(ConfigError::Read { .. })
    ));
    assert!(!missing.exists());
    let existing = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../editor.toml");
    let before = std::fs::read(&existing).unwrap();
    assert_eq!(
        EditorConfig::load(&existing).unwrap(),
        EditorConfig::glyphfolio()
    );
    assert_eq!(std::fs::read(existing).unwrap(), before);
}

#[test]
fn an_empty_toolbar_and_rich_only_configuration_are_valid() {
    let config = EditorConfig::from_toml(
        r#"
        [editor]
        default_presentation = "rich"
        allow_experimental_rich = true
        [toolbar]
        groups = []
        [features]
        source = false
        "#,
    )
    .unwrap();
    assert!(
        !config
            .features
            .allows_presentation(PresentationMode::Source)
    );
    assert!(config.features.allows_presentation(PresentationMode::Rich));
    assert!(config.toolbar.groups.is_empty());
    assert_eq!(
        PresentationMode::from(Presentation::Rich),
        PresentationMode::Rich
    );
}
