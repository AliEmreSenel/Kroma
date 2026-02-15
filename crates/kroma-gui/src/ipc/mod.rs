//! Async IPC module — non-blocking communication with kroma-daemon.
//!
//! Architecture:
//! - A background tokio task maintains a persistent Unix socket connection.
//! - Commands are sent via an `mpsc` channel from the GUI thread.
//! - Events (status, compile results, preview frames) arrive via an iced
//!   `Subscription` and are dispatched as `Message`s.
//! - Auto-reconnect with exponential backoff on disconnection.

mod client;

pub use client::{ipc_subscription, IpcEvent, IpcHandle};
