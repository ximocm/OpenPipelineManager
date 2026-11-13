//! Marketplace installs definitions; the palette paints ordinary self-contained steps.
pub mod catalog;
mod palette;
mod window;

pub use palette::{configure, ToolPalette};
pub use window::open;

pub fn tools_root() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("OPM_TOOLS_DIR").filter(|p| !p.is_empty()) {
        return path.into();
    }
    directories::BaseDirs::new()
        .map(|d| d.data_dir().to_owned())
        .unwrap_or_else(std::env::temp_dir)
        .join("opm")
        .join("tools")
}

pub fn tool_library() -> opm_store::ToolLibrary {
    opm_store::ToolLibrary::new(tools_root())
}
