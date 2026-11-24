//! Connect and disconnect, on each side. The engines are behind traits so the flows can be
//! tested without Sunshine, Moonlight or a Mac.

pub mod client;
pub mod engines;
pub mod host;

use crate::config::StreamSettings;
use crate::sunshine::EngineError;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Disconnected,
    Connecting,
    Connected,
    Failed { part: String, reason: String },
}

pub type StateTx = tokio::sync::watch::Sender<State>;

/// Everything the Mac runs for a session: the virtual display and the stream host.
#[async_trait]
pub trait HostEngines: Send {
    async fn start(&mut self, width: u32, height: u32, fps: u32) -> Result<(), EngineError>;
    /// `Ok(false)` means the viewer has not asked to pair yet.
    async fn submit_pin(&mut self, pin: &str, client_name: &str) -> Result<bool, EngineError>;
    /// Safe to call when nothing is running, and more than once.
    async fn stop(&mut self);
    /// Resolves with the name of a part that stopped by itself.
    async fn exited(&mut self) -> &'static str;
}

/// The viewer window on the PC.
#[async_trait]
pub trait ViewerEngine: Send {
    /// Whether the viewer still counts itself paired with the stream host at `host`; `None` when
    /// it cannot tell. Asked only while the host is running.
    async fn is_paired(&mut self, host: &str) -> std::io::Result<Option<bool>>;
    async fn pair(&mut self, host: &str, pin: &str) -> std::io::Result<bool>;
    async fn start(&mut self, host: &str, s: &StreamSettings) -> std::io::Result<()>;
    /// Resolves when the viewer window exits, or the viewer puts up an error ([`last_words`]).
    ///
    /// [`last_words`]: ViewerEngine::last_words
    async fn closed(&mut self);
    /// The error the viewer put up for the user, if that is why [`closed`](ViewerEngine::closed)
    /// resolved (taken once): the session fails with it, and the viewer (with its message) goes.
    fn last_words(&mut self) -> Option<String> {
        None
    }
    async fn stop(&mut self);
}
