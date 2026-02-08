//! # Kroma Shared
//!
//! Core library for the Kroma wallpaper engine.
//! Contains all trait definitions, shared types, IPC protocol,
//! `.shade` package format, and the Shadertoy-to-Kroma shader translator.

pub mod error;
pub mod ipc;
pub mod mock;
pub mod shade;
pub mod traits;
pub mod translator;
pub mod types;
