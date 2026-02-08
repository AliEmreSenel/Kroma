//! Video decoder placeholder.
//!
//! Implements [`kroma_shared::traits::VideoDecoder`] as a stub.
//! The real implementation will use `ffmpeg-next` or `gstreamer-rs`.

#![allow(dead_code)]

use std::path::Path;

use anyhow::Result;

use kroma_shared::traits::VideoDecoder;

/// Stub video decoder — returns a single-pixel magenta frame.
///
/// Will be replaced with a real ffmpeg/gstreamer implementation.
pub struct StubVideoDecoder {
    width: u32,
    height: u32,
    frame_buffer: Vec<u8>,
}

impl VideoDecoder for StubVideoDecoder {
    fn load(path: &Path) -> Result<Self>
    where
        Self: Sized,
    {
        log::warn!(
            "StubVideoDecoder: loading '{}' — real decoder not yet implemented",
            path.display()
        );

        // Return a tiny 1×1 magenta frame so the pipeline stays functional
        let width = 1;
        let height = 1;
        let frame_buffer = vec![255, 0, 255, 255]; // RGBA magenta

        Ok(Self {
            width,
            height,
            frame_buffer,
        })
    }

    fn next_frame(&mut self) -> Option<&[u8]> {
        Some(&self.frame_buffer)
    }

    fn seek(&mut self, _timestamp: f64) -> Result<()> {
        // No-op for stub
        Ok(())
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}
