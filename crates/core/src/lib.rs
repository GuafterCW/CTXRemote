//! Engine behind the CTXRemote app: hosting sessions and viewing remote screens.

pub mod capture;
pub mod config;
pub mod encoder;
pub mod host;
pub mod input;
pub mod keymap;
mod net;
pub mod viewer;

pub use ctxremote_proto as proto;
