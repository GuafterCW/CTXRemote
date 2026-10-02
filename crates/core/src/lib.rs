//! Engine behind the CTXRemote app: hosting sessions and viewing remote screens.

pub mod agent;
pub mod agent_process;
pub mod capture;
pub mod clipboard;
pub mod config;
pub mod desktop;
pub mod encoder;
pub mod host;
pub mod input;
pub mod keymap;
mod net;
pub mod sas;
pub mod ui_link;
pub mod viewer;

pub use ctxremote_proto as proto;
