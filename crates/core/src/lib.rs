//! Engine behind the CTXRemote app: hosting sessions and viewing remote screens.

pub mod agent;
pub mod alias;
pub mod agent_process;
pub mod capture;
pub mod clipboard;
pub mod config;
mod congestion;
pub mod desktop;
pub mod direct;
pub mod encoder;
pub mod files;
pub mod host;
pub mod input;
pub mod keymap;
mod net;
pub mod sas;
pub mod ui_link;
pub mod update;
pub mod viewer;

pub use ctxremote_proto as proto;
