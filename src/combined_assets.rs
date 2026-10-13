use crate::assets::CustomAssets;
use gpui::{AssetSource, Result, SharedString};
use gpui_component_assets::Assets as GpuiComponentAssets;
use std::borrow::Cow;

pub struct CombinedAssets {
    gpui_assets: GpuiComponentAssets,
    custom_assets: CustomAssets,
}

impl CombinedAssets {
    pub fn new() -> Self {
        Self {
            gpui_assets: GpuiComponentAssets,
            custom_assets: CustomAssets,
        }
    }
}

impl AssetSource for CombinedAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        // Try loading from custom assets first
        if let Some(data) = self.custom_assets.load(path)? {
            return Ok(Some(data));
        }
        // If not found in custom assets, try loading from gpui_component assets
        self.gpui_assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut list = self.custom_assets.list(path)?;
        list.extend(self.gpui_assets.list(path)?);
        Ok(list)
    }
}
