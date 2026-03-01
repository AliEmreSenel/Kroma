//! Video texture source.
//!
//! Wraps an FFmpeg decoder and produces RGBA frames at the video's
//! native frame rate. Handles accumulator-based timing and looping.

use std::path::Path;

use anyhow::Result;
use log::info;

use kroma_shared::traits::VideoDecoder;

use super::{TextureSource, TextureUpdate};

/// Re-export the FFmpeg decoder.
pub use crate::video::FfmpegVideoDecoder;

/// A video texture that decodes frames on its own schedule.
pub struct VideoTexture {
    decoder: FfmpegVideoDecoder,
    /// Frame-rate accumulator (seconds).
    accum: f64,
    /// Whether to loop the video on EOF.
    looping: bool,
    /// True on the first update (forces an initial frame decode).
    pub(crate) first_frame: bool,
    /// Cached last frame data for dimensions.
    last_width: u32,
    last_height: u32,
}

impl VideoTexture {
    /// Open a video file and prepare the decoder.
    pub fn load(path: &Path, looping: bool) -> Result<Self> {
        let decoder = FfmpegVideoDecoder::load(path)?;
        let (w, h) = decoder.dimensions();
        info!(
            "VideoTexture loaded: {}x{}, looping={}",
            w, h, looping
        );
        Ok(Self {
            decoder,
            accum: 0.0,
            looping,
            first_frame: true,
            last_width: w,
            last_height: h,
        })
    }

    /// Decode one frame, handling looping.
    fn decode_one_frame(&mut self) -> Option<Vec<u8>> {
        let (w, h) = self.decoder.dimensions();
        self.last_width = w;
        self.last_height = h;
        match self.decoder.next_frame() {
            Some(data) => Some(data.to_vec()),
            None => {
                if self.looping {
                    if let Err(e) = self.decoder.seek(0.0) {
                        log::warn!("Video seek-to-start failed: {}", e);
                    }
                    // Try again after seeking
                    self.decoder.next_frame().map(|d| d.to_vec())
                } else {
                    None
                }
            }
        }
    }

    fn frame_interval(&self) -> f64 {
        let raw = self.decoder.frame_interval();
        if raw.is_finite() && raw > 0.0 {
            raw
        } else {
            1.0 / 30.0
        }
    }
}

impl TextureSource for VideoTexture {
    fn update(&mut self, dt: f64) -> Result<TextureUpdate> {
        // On first frame, decode immediately regardless of timing.
        if self.first_frame {
            self.first_frame = false;
            if let Some(data) = self.decode_one_frame() {
                return Ok(TextureUpdate::NewFrame {
                    width: self.last_width,
                    height: self.last_height,
                    data,
                });
            }
            return Ok(TextureUpdate::Unchanged);
        }

        let interval = self.frame_interval();
        self.accum += dt;

        // Only produce one frame per update even if multiple intervals elapsed
        // to avoid decoding too many frames when the render loop hitches.
        let mut latest_frame: Option<Vec<u8>> = None;
        while self.accum >= interval {
            self.accum -= interval;
            if let Some(data) = self.decode_one_frame() {
                latest_frame = Some(data);
            }
        }

        match latest_frame {
            Some(data) => Ok(TextureUpdate::NewFrame {
                width: self.last_width,
                height: self.last_height,
                data,
            }),
            None => Ok(TextureUpdate::Unchanged),
        }
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.last_width, self.last_height)
    }

    fn texture_type(&self) -> &'static str {
        "video"
    }
}
