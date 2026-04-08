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
    opaque: *mut StreamOpaque,
}

impl InputContext {
    pub fn from_input(input: format::context::Input) -> Self {
        Self {
            input: ManuallyDrop::new(input),
            opaque: std::ptr::null_mut(),
        }
    }

    pub fn open_from_stream(stream: AssetByteStream) -> Result<Self> {
        let opaque = Box::new(StreamOpaque {
            stream,
            position: 0,
        });
        let opaque_ptr = Box::into_raw(opaque);

        let avio_buffer_size = 64 * 1024usize;
        // SAFETY: FFmpeg requires av_malloc-allocated buffer for AVIO.
        let avio_buffer = unsafe { ffi::av_malloc(avio_buffer_size) as *mut u8 };
        if avio_buffer.is_null() {
            // SAFETY: opaque_ptr came from Box::into_raw above and has not been freed.
            unsafe {
                drop(Box::from_raw(opaque_ptr));
            }
            anyhow::bail!("Failed to allocate AVIO buffer")
        }

        // SAFETY: Callbacks and opaque pointer remain valid until InputContext::drop.
        let avio_ctx = unsafe {
            ffi::avio_alloc_context(
                avio_buffer,
                avio_buffer_size as i32,
                0,
                opaque_ptr.cast::<c_void>(),
                Some(read_packet),
                None,
                Some(seek_stream),
            )
        };
        if avio_ctx.is_null() {
            // SAFETY: Free buffer and opaque if avio context creation failed.
            unsafe {
                ffi::av_free(avio_buffer.cast::<c_void>());
                drop(Box::from_raw(opaque_ptr));
            }
            anyhow::bail!("Failed to allocate FFmpeg AVIO context")
        }

        // SAFETY: Allocates a fresh AVFormatContext.
        let mut fmt_ctx = unsafe { ffi::avformat_alloc_context() };
        if fmt_ctx.is_null() {
            // SAFETY: Free AVIO context and opaque on failure.
            unsafe {
                let mut avio_to_free = avio_ctx;
                ffi::avio_context_free(&mut avio_to_free);
                drop(Box::from_raw(opaque_ptr));
            }
            anyhow::bail!("Failed to allocate FFmpeg format context")
        }

        // SAFETY: Assign custom IO context to format context.
        unsafe {
            (*fmt_ctx).pb = avio_ctx;
            (*fmt_ctx).flags |= ffi::AVFMT_FLAG_CUSTOM_IO;
        }

        // SAFETY: Open input using already attached custom AVIO context.
        let open_result = unsafe {
            ffi::avformat_open_input(
                &mut fmt_ctx,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };

        if open_result < 0 {
            // SAFETY: Manually free context, attached AVIO and opaque state.
            unsafe {
                let mut avio_to_free = (*fmt_ctx).pb;
                ffi::avio_context_free(&mut avio_to_free);
                ffi::avformat_free_context(fmt_ctx);
                drop(Box::from_raw(opaque_ptr));
            }
            anyhow::bail!("Failed to open FFmpeg input from embedded stream")
        }

        // SAFETY: Collect stream metadata for demuxing.
        let stream_info_result =
            unsafe { ffi::avformat_find_stream_info(fmt_ctx, std::ptr::null_mut()) };
        if stream_info_result < 0 {
            // SAFETY: Close input and release opaque state.
            unsafe {
                ffi::avformat_close_input(&mut fmt_ctx);
                drop(Box::from_raw(opaque_ptr));
            }
            anyhow::bail!("Failed to read FFmpeg stream info from embedded stream")
        }

        // SAFETY: fmt_ctx is now owned by ffmpeg-next Input wrapper.
        let input = unsafe { format::context::Input::wrap(fmt_ctx) };

        Ok(Self {
            input: ManuallyDrop::new(input),
            opaque: opaque_ptr,
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
        // SAFETY: Drop FFmpeg input first so callback usage is finished,
        // then reclaim opaque callback state.
        unsafe {
            ManuallyDrop::drop(&mut self.input);
            if !self.opaque.is_null() {
                drop(Box::from_raw(self.opaque));
                self.opaque = std::ptr::null_mut();
            }
        }
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
