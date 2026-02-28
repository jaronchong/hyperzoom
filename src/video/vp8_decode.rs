/// VP8 decoder wrapper using raw `env-libvpx-sys` FFI.
use std::ptr;
use std::slice;

use vpx_sys as vpx;

/// A decoded I420 frame.
pub struct DecodedFrame {
    pub data: Vec<u8>, // I420 planar: Y + U + V
    pub width: u32,
    pub height: u32,
}

pub struct Vp8Decoder {
    ctx: vpx::vpx_codec_ctx_t,
}

// SAFETY: The decoder context is only used from a single task/thread at a time.
// vpx_codec_ctx_t contains internal pointers that are not inherently Send,
// but exclusive access guarantees safety.
unsafe impl Send for Vp8Decoder {}

impl Vp8Decoder {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let mut ctx: vpx::vpx_codec_ctx_t = std::mem::zeroed();
            let iface = vpx::vpx_codec_vp8_dx();
            if iface.is_null() {
                return Err("vpx_codec_vp8_dx returned null".into());
            }

            let cfg = vpx::vpx_codec_dec_cfg_t {
                threads: 1,
                w: 0,
                h: 0,
            };

            let err = vpx::vpx_codec_dec_init_ver(
                &mut ctx,
                iface,
                &cfg,
                0,
                vpx::VPX_DECODER_ABI_VERSION as i32,
            );

            if err != vpx::VPX_CODEC_OK {
                return Err(format!("vpx_codec_dec_init_ver failed: error {err:?}"));
            }

            Ok(Self { ctx })
        }
    }

    /// Decode a VP8 packet. Returns the decoded I420 frame if available.
    pub fn decode(&mut self, data: &[u8]) -> Result<Option<DecodedFrame>, String> {
        unsafe {
            let err = vpx::vpx_codec_decode(
                &mut self.ctx,
                data.as_ptr(),
                data.len() as u32,
                ptr::null_mut(),
                0, // deadline: 0 = best quality
            );

            if err != vpx::VPX_CODEC_OK {
                return Err(format!("vpx_codec_decode failed: error {err:?}"));
            }

            // Retrieve decoded frame
            let mut iter: vpx::vpx_codec_iter_t = ptr::null();
            let img = vpx::vpx_codec_get_frame(&mut self.ctx, &mut iter);

            if img.is_null() {
                return Ok(None);
            }

            let img = &*img;
            let w = img.d_w as usize;
            let h = img.d_h as usize;
            let uv_w = w / 2;
            let uv_h = h / 2;

            let y_stride = img.stride[0] as usize;
            let u_stride = img.stride[1] as usize;
            let v_stride = img.stride[2] as usize;

            let total = w * h + 2 * uv_w * uv_h;
            let mut yuv = Vec::with_capacity(total);

            // Copy Y plane
            for row in 0..h {
                let src = slice::from_raw_parts(img.planes[0].add(row * y_stride), w);
                yuv.extend_from_slice(src);
            }

            // Copy U plane
            for row in 0..uv_h {
                let src = slice::from_raw_parts(img.planes[1].add(row * u_stride), uv_w);
                yuv.extend_from_slice(src);
            }

            // Copy V plane
            for row in 0..uv_h {
                let src = slice::from_raw_parts(img.planes[2].add(row * v_stride), uv_w);
                yuv.extend_from_slice(src);
            }

            Ok(Some(DecodedFrame {
                data: yuv,
                width: w as u32,
                height: h as u32,
            }))
        }
    }
}

impl Drop for Vp8Decoder {
    fn drop(&mut self) {
        unsafe {
            vpx::vpx_codec_destroy(&mut self.ctx);
        }
    }
}
