//! Asset source: the app's own icons and theme first, then gpui-kit's defaults.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

macro_rules! embedded {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_bytes!(concat!("../assets/", $path)) as &[u8])),*]
    };
}

/// Files bundled into the binary, by asset path.
static EMBEDDED: &[(&str, &[u8])] = embedded![
    "demo/cursor.svg",
    "icons/brain.svg",
    "icons/check-check.svg",
    "icons/sparkles.svg",
    "icons/code-xml.svg",
    "icons/corner-down-right.svg",
    "icons/download.svg",
    "icons/files.svg",
    "icons/git-branch.svg",
    "icons/git-compare.svg",
    "icons/globe.svg",
    "icons/globe-lock.svg",
    "icons/globe-off.svg",
    "icons/wifi-off.svg",
    "icons/hand-helping.svg",
    "icons/image.svg",
    "icons/link.svg",
    "icons/message-square.svg",
    "icons/package.svg",
    "icons/paperclip.svg",
    "icons/pencil.svg",
    "icons/pi-logo.svg",
    "icons/play.svg",
    "icons/rotate-ccw.svg",
    "icons/square-pen.svg",
    "icons/text-wrap.svg",
    "icons/wrench.svg",
    "icons/external-link.svg",
    "icons/scroll-text.svg",
    "icons/gauge.svg",
    "icons/gpu.svg",
    "icons/hard-drive.svg",
    "icons/laptop.svg",
    "icons/memory-stick.svg",
    "icons/plug.svg",
    "icons/book-open.svg",
    "icons/bug.svg",
    "icons/chart-column.svg",
    "icons/database.svg",
    "icons/file-text.svg",
    "icons/flask-conical.svg",
    "icons/key.svg",
    "icons/library.svg",
    "icons/palette.svg",
    "icons/puzzle.svg",
    "icons/rocket.svg",
    "icons/server.svg",
    "icons/shield-check.svg",
    "icons/shield-off.svg",
    "icons/trash.svg",
    "themes/pi.json",
];

pub struct AppAssets;

impl AppAssets {
    pub fn get(path: &str) -> Option<&'static [u8]> {
        EMBEDDED
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| *bytes)
    }
}

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = Self::get(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut entries = gpui_kit::assets::Assets.list(path)?;
        entries.extend(
            EMBEDDED
                .iter()
                .filter(|(name, _)| name.starts_with(path))
                .map(|(name, _)| SharedString::from(*name)),
        );
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_assets_resolve_and_theme_parses() {
        assert!(AppAssets::get("icons/paperclip.svg").is_some());
        let theme = AppAssets::get("themes/pi.json").unwrap();
        let value: serde_json::Value = serde_json::from_slice(theme).unwrap();
        assert_eq!(value["themes"].as_array().unwrap().len(), 2);
    }
}
