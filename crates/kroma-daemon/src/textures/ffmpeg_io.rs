//! FFmpeg custom IO bridge for embedded chunk-streamed video assets.
//!
//! All unsafe FFmpeg AVIO interop is intentionally contained in this module.

use std::ffi::c_void;
use std::mem::ManuallyDrop;

use anyhow::Result;
use ffmpeg_next::{ffi, format};

use kroma_shared::shade::AssetByteStream;

struct StreamOpaque {
    stream: AssetByteStream,
    position: u64,
}

pub struct InputContext {
    input: ManuallyDrop<format::context::Input>,
    // Fields drop after the input, in order: AVIO before its callback state.
    _avio: Option<OwnedAvio>,
    _opaque: Option<Box<StreamOpaque>>,
}

/// Owns custom IO separately: avformat_close_input never releases custom AVIO.
struct OwnedAvio(*mut ffi::AVIOContext);

impl Drop for OwnedAvio {
    fn drop(&mut self) {
        // SAFETY: This uniquely owns the context. FFmpeg may replace its buffer,
        // so free the current buffer, then the context, after closing the input.
        unsafe {
            ffi::av_free((*self.0).buffer.cast::<c_void>());
            ffi::avio_context_free(&mut self.0);
        }
    }
}

impl InputContext {
    pub fn from_input(input: format::context::Input) -> Self {
        Self {
            input: ManuallyDrop::new(input),
            _avio: None,
            _opaque: None,
        }
    }

    pub fn open_from_stream(stream: AssetByteStream) -> Result<Self> {
        let _timing = crate::timing::scope("video.embedded.open");
        let mut opaque = Box::new(StreamOpaque {
            stream,
            position: 0,
        });
        let avio_buffer_size = 64 * 1024usize;
        // SAFETY: FFmpeg requires an av_malloc-allocated buffer for AVIO.
        let avio_buffer = unsafe { ffi::av_malloc(avio_buffer_size) as *mut u8 };
        anyhow::ensure!(!avio_buffer.is_null(), "Failed to allocate AVIO buffer");

        // SAFETY: Box keeps callback state stable until after the input and AVIO
        // close. On allocation failure FFmpeg has not taken the buffer.
        let avio_ctx = unsafe {
            ffi::avio_alloc_context(
                avio_buffer,
                avio_buffer_size as i32,
                0,
                (&mut *opaque as *mut StreamOpaque).cast::<c_void>(),
                Some(read_packet),
                None,
                Some(seek_stream),
            )
        };
        if avio_ctx.is_null() {
            // SAFETY: Allocation failed, so the buffer is still ours.
            unsafe { ffi::av_free(avio_buffer.cast::<c_void>()) };
            anyhow::bail!("Failed to allocate FFmpeg AVIO context");
        }
        let avio = OwnedAvio(avio_ctx);
        // SAFETY: Allocates a fresh context; avio's owner handles all error paths.
        let mut fmt_ctx = unsafe { ffi::avformat_alloc_context() };
        anyhow::ensure!(
            !fmt_ctx.is_null(),
            "Failed to allocate FFmpeg format context"
        );
        // SAFETY: Both contexts are valid and remain alive throughout opening.
        unsafe {
            (*fmt_ctx).pb = avio.0;
            (*fmt_ctx).flags |= ffi::AVFMT_FLAG_CUSTOM_IO;
        }
        // SAFETY: FFmpeg may free and null fmt_ctx on failure. The custom IO
        // remains independently owned and must never be recovered via fmt_ctx.
        let result = unsafe {
            ffi::avformat_open_input(
                &mut fmt_ctx,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if result < 0 {
            // SAFETY: close_input accepts null and clears a surviving context.
            unsafe { ffi::avformat_close_input(&mut fmt_ctx) };
            anyhow::bail!("Failed to open FFmpeg input from embedded stream");
        }
        // SAFETY: Open context is valid and callback state is still alive.
        let result = unsafe { ffi::avformat_find_stream_info(fmt_ctx, std::ptr::null_mut()) };
        if result < 0 {
            // SAFETY: Close input before dropping its custom IO and callback state.
            unsafe { ffi::avformat_close_input(&mut fmt_ctx) };
            anyhow::bail!("Failed to read FFmpeg stream info from embedded stream");
        }
        // SAFETY: Transfer the successfully opened format context to ffmpeg-next.
        let input = unsafe { format::context::Input::wrap(fmt_ctx) };
        Ok(Self {
            input: ManuallyDrop::new(input),
            _avio: Some(avio),
            _opaque: Some(opaque),
        })
    }

    pub fn as_input(&self) -> &format::context::Input {
        &self.input
    }

    pub fn as_input_mut(&mut self) -> &mut format::context::Input {
        &mut self.input
    }
}

impl Drop for InputContext {
    fn drop(&mut self) {
        // SAFETY: Close input before the owned AVIO and callback state fields drop.
        unsafe { ManuallyDrop::drop(&mut self.input) };
    }
}

unsafe extern "C" fn read_packet(opaque: *mut c_void, buf: *mut u8, buf_size: i32) -> i32 {
    if opaque.is_null() || buf.is_null() || buf_size <= 0 {
        return -1;
    }

    let state = unsafe { &mut *(opaque.cast::<StreamOpaque>()) };
    let out = unsafe { std::slice::from_raw_parts_mut(buf, buf_size as usize) };

    match state.stream.read_at(state.position, out) {
        Ok(0) => ffi::AVERROR_EOF,
        Ok(n) => {
            state.position = state.position.saturating_add(n as u64);
            n as i32
        }
        Err(_) => -1,
    }
}

unsafe extern "C" fn seek_stream(opaque: *mut c_void, offset: i64, whence: i32) -> i64 {
    if opaque.is_null() {
        return -1;
    }

    let state = unsafe { &mut *(opaque.cast::<StreamOpaque>()) };
    let len = state.stream.len() as i64;

    if whence == ffi::AVSEEK_SIZE {
        return len;
    }

    let base = match whence {
        0 => 0i64,                  // SEEK_SET
        1 => state.position as i64, // SEEK_CUR
        2 => len,                   // SEEK_END
        _ => return -1,
    };

    let new_pos = base.saturating_add(offset);
    if new_pos < 0 {
        return -1;
    }

    state.position = new_pos as u64;
    state.position as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(bytes: Vec<u8>) -> AssetByteStream {
        let config = toml::from_str("[meta]\nname='IO test'\nauthor='test'\nversion='1'").unwrap();
        let mut package = kroma_shared::shade::LiveShadePackage::new_empty(config);
        package.add_asset("fixture".into(), bytes);
        package.open_asset_stream("fixture").unwrap()
    }

    #[test]
    fn invalid_embedded_input_returns_error_without_dereferencing_freed_context() {
        ffmpeg_next::init().unwrap();
        for bytes in [vec![], vec![0; 128], b"invalid video".to_vec()] {
            assert!(InputContext::open_from_stream(stream(bytes)).is_err());
        }
    }

    #[test]
    fn custom_input_can_open_seek_and_close_repeatedly() {
        ffmpeg_next::init().unwrap();
        // A small PCM WAV exercises successful custom input without external files.
        let mut wav = b"RIFF".to_vec();
        wav.extend(52u32.to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(8000u32.to_le_bytes());
        wav.extend(16000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(16u32.to_le_bytes());
        wav.extend([0; 16]);
        for _ in 0..64 {
            let mut input = InputContext::open_from_stream(stream(wav.clone())).unwrap();
            assert_eq!(input.as_input().streams().count(), 1);
            input.as_input_mut().seek(0, ..).unwrap();
            assert!(input.as_input_mut().packets().next().is_some());
        }
    }
}
