//! Custom error types for Kroma.

use thiserror::Error;

/// Top-level error type for the Kroma engine.
#[derive(Debug, Error)]
pub enum KromaError {
    #[error("Windowing error: {0}")]
    Windowing(String),

    #[error("Render error: {0}")]
    Render(String),

    #[error("Shader compilation error: {0}")]
    ShaderCompilation(String),

    #[error("Shader translation error: {0}")]
    ShaderTranslation(String),

    #[error("Video decoder error: {0}")]
    VideoDecoder(String),

    #[error("Data provider error: {0}")]
    DataProvider(String),

    #[error("IPC error: {0}")]
    Ipc(String),

    #[error(".shade package error: {0}")]
    ShadePackage(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),
}
