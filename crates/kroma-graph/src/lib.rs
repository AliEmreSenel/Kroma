//! Shader node graph — types, GLSL parser, and code generator.
//!
//! This crate provides a framework-agnostic node graph that represents
//! a Shadertoy-compatible GLSL fragment shader.  It can:
//!
//! - **Parse** GLSL into a node graph ([`lowering::parse_glsl_to_graph`])
//! - **Compile** a node graph back to GLSL ([`ShaderGraph::compile_glsl`])
//!
//! The GUI-specific rendering (canvas, widget, etc.) is **not** in this crate.

pub mod graph;
pub mod lowering;
pub mod nodes;
pub mod types;

// Re-export the most-used items at crate root.
pub use graph::ShaderGraph;
pub use lowering::parse_glsl_to_graph;
pub use types::*;
