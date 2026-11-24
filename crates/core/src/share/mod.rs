//! Mouse and keyboard sharing: the PC's mouse and keyboard drive the Mac, one way.

pub mod admit;
pub mod channel;
pub mod follower;
pub mod geometry;
pub mod keys;
pub mod wire;

#[cfg(windows)]
pub mod pc;

#[cfg(target_os = "macos")]
pub mod mac;
