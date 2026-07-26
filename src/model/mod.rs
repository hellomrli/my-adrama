//! Domain layer: plain data + disk layout. No HTTP, no credentials, no UI.

pub mod breakdown;
pub mod config;
pub mod index;
pub mod project;
pub mod state;

pub use breakdown::{
    AssetMeta, Breakdown, Character, Costume, Location, Prop, Shot, StoryboardMeta, VideoMeta,
};
pub use config::{
    AspectRatio, Capability, Endpoint, EndpointMode, GenerationSettings, ProjectConfig, ProviderId,
};
pub use index::{ItemView, ProjectIndex};
pub use project::{AssetKind, Project, ASSET_KINDS};
pub use state::{ItemStatus, ProjectState, Stage, StageStatus};
