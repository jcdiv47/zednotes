use gpui::Pixels;
use settings::{IntoGpui, RegisterSetting, Settings};

/// The settings for the markdown preview.
#[derive(Clone, Copy, Debug, RegisterSetting)]
pub struct MarkdownPreviewSettings {
    /// The maximum width of the rendered markdown content, or `None` to render
    /// content edge to edge.
    pub max_width: Option<Pixels>,
    /// Whether images referenced by HTTP or HTTPS URLs may be loaded.
    pub allow_remote_images: bool,
    /// The maximum source size that renders without explicit confirmation.
    pub max_file_size_bytes: Option<u64>,
}

impl Default for MarkdownPreviewSettings {
    fn default() -> Self {
        Self {
            max_width: None,
            allow_remote_images: true,
            max_file_size_bytes: None,
        }
    }
}

impl Settings for MarkdownPreviewSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let content = content.markdown_preview.clone().unwrap_or_default();
        let max_width = if content.limit_content_width.unwrap_or(true) {
            content.max_width.map(IntoGpui::into_gpui)
        } else {
            None
        };
        Self {
            max_width,
            allow_remote_images: content.allow_remote_images.unwrap_or(true),
            max_file_size_bytes: content.max_file_size_bytes,
        }
    }
}
