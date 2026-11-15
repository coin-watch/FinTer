use crate::data::client::ApiClient;
use crate::data::error::Result;
use crate::data::models::LayoutPreset;

impl ApiClient {
    /// List all layout presets.
    ///
    /// Presets are the same on-disk dashboards as `modules` — a preset is just a
    /// dashboard that may be marked public. Kept as a separate accessor so the
    /// existing UI code that treats presets and dashboards differently keeps
    /// compiling.
    ///
    /// Blocking — run from a background thread.
    pub fn list_layout_presets(&self) -> Result<Vec<LayoutPreset>> {
        crate::data::dashboard_files::list_presets()
    }

    /// Create a new layout preset.
    ///
    /// Blocking — run from a background thread.
    pub fn create_layout_preset(
        &self,
        name: &str,
        tree: serde_json::Value,
    ) -> Result<LayoutPreset> {
        let dashboard = crate::data::dashboard_files::create(name, tree)?;
        Ok(dashboard.to_preset())
    }

    /// Delete a layout preset.
    ///
    /// Blocking — run from a background thread.
    pub fn delete_layout_preset(&self, preset_id: &str) -> Result<()> {
        crate::data::dashboard_files::delete(preset_id)
    }
}
