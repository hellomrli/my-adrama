//! One module per screen. Views read from [`AppState`], draw, and queue
//! actions; they never call the engine directly except through `state.submit`.

pub mod breakdown;
pub mod dashboard;
pub mod flow;
pub mod script;
pub mod settings;
pub mod workbench;

use super::runtime::Runtime;
use super::state::AppState;
use super::thumbs::Thumbnails;

pub struct ViewCtx<'a> {
    pub state: &'a mut AppState,
    pub runtime: &'a Runtime,
    pub thumbs: &'a mut Thumbnails,
}
