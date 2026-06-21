//! # Kroma Shared
//!
//! Core library for the Kroma wallpaper engine.
//! Contains all trait definitions, shared types, IPC protocol,
//! `.shade` package format, and the Shadertoy-to-Kroma shader translator.

pub mod compression;
pub mod error;
pub mod ipc;
pub mod shade;
pub mod traits;
pub mod transition;
pub mod translator;
pub mod types;
