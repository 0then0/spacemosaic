//! Storage metadata, background scanning, and treemap geometry for SpaceMosaic.
pub mod bridge;
pub mod i18n;
pub mod scan;
#[cfg(feature = "gui")]
pub mod theme;
pub mod treemap;
#[cfg(feature = "gui")]
pub mod ui;

#[cfg(all(feature = "gui", target_os = "android"))]
mod android;
#[cfg(all(feature = "gui", target_os = "ios"))]
mod ios;
