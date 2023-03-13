//! Reusable change selector UI for source control systems.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

pub mod consts;
mod render;
mod types;
mod ui;
mod util;

pub use types::{
    ChangeType, File, FileMode, RecordError, RecordState, Section, SectionChangedLine,
};
pub use ui::{Event, EventSource, Recorder, TestingScreenshot};
