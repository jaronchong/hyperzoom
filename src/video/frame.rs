use fast_image_resize as fir;

/// A raw RGB24 video frame.
#[derive(Clone)]
pub struct VideoFrame {
    pub data: Vec<u8>, // RGB24: width * height * 3 bytes
    pub width: u32,
    pub height: u32,
}

/// Convert RGB24 data to I420 (YUV420 planar) using BT.601 coefficients.
///
/// Output layout: Y plane (w*h) + U plane (w/2 * h/2) + V plane (w/2 * h/2)
pub fn rgb_to_i420(rgb: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let uv_w = w / 2;
    let uv_h = h / 2;

    let y_size = w * h;
    let uv_size = uv_w * uv_h;
    let mut yuv = vec![0u8; y_size + 2 * uv_size];

    let (y_plane, uv_planes) = yuv.split_at_mut(y_size);
    let (u_plane, v_plane) = uv_planes.split_at_mut(uv_size);

    // Y plane — every pixel
    for row in 0..h {
        for col in 0..w {
            let idx = (row * w + col) * 3;
            let r = rgb[idx] as f32;
            let g = rgb[idx + 1] as f32;
            let b = rgb[idx + 2] as f32;
            let y = 16.0 + 65.481 * r / 255.0 + 128.553 * g / 255.0 + 24.966 * b / 255.0;
            y_plane[row * w + col] = y.clamp(0.0, 255.0) as u8;
        }
    }

    // U and V planes — subsampled 2x2
    for row in 0..uv_h {
        for col in 0..uv_w {
            // Average the 2x2 block
            let mut r_sum = 0u32;
            let mut g_sum = 0u32;
            let mut b_sum = 0u32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let idx = ((row * 2 + dy) * w + col * 2 + dx) * 3;
                    r_sum += rgb[idx] as u32;
                    g_sum += rgb[idx + 1] as u32;
                    b_sum += rgb[idx + 2] as u32;
                }
            }
            let r = (r_sum / 4) as f32;
            let g = (g_sum / 4) as f32;
            let b = (b_sum / 4) as f32;

            let u =
                128.0 - 37.797 * r / 255.0 - 74.203 * g / 255.0 + 112.0 * b / 255.0;
            let v =
                128.0 + 112.0 * r / 255.0 - 93.786 * g / 255.0 - 18.214 * b / 255.0;

            u_plane[row * uv_w + col] = u.clamp(0.0, 255.0) as u8;
            v_plane[row * uv_w + col] = v.clamp(0.0, 255.0) as u8;
        }
    }

    yuv
}

/// Convert I420 (YUV420 planar) data back to RGB24 using BT.601 coefficients.
pub fn i420_to_rgb(yuv: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let uv_w = w / 2;

    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);

    let y_plane = &yuv[..y_size];
    let u_plane = &yuv[y_size..y_size + uv_size];
    let v_plane = &yuv[y_size + uv_size..];

    let mut rgb = vec![0u8; w * h * 3];

    for row in 0..h {
        for col in 0..w {
            let y = y_plane[row * w + col] as f32 - 16.0;
            let u = u_plane[(row / 2) * uv_w + col / 2] as f32 - 128.0;
            let v = v_plane[(row / 2) * uv_w + col / 2] as f32 - 128.0;

            let r = 1.164 * y + 1.596 * v;
            let g = 1.164 * y - 0.392 * u - 0.813 * v;
            let b = 1.164 * y + 2.017 * u;

            let idx = (row * w + col) * 3;
            rgb[idx] = r.clamp(0.0, 255.0) as u8;
            rgb[idx + 1] = g.clamp(0.0, 255.0) as u8;
            rgb[idx + 2] = b.clamp(0.0, 255.0) as u8;
        }
    }

    rgb
}

/// Downscale an RGB frame to the target resolution using fast_image_resize.
pub fn downscale_rgb(src: &VideoFrame, target_w: u32, target_h: u32) -> VideoFrame {
    // If already at target size, return a clone
    if src.width == target_w && src.height == target_h {
        return src.clone();
    }

    let src_image = fir::images::Image::from_vec_u8(
        src.width,
        src.height,
        src.data.clone(),
        fir::PixelType::U8x3,
    )
    .expect("invalid source image dimensions");

    let mut dst_image = fir::images::Image::new(target_w, target_h, fir::PixelType::U8x3);

    let mut resizer = fir::Resizer::new();
    resizer
        .resize(&src_image, &mut dst_image, None)
        .expect("resize failed");

    VideoFrame {
        data: dst_image.into_vec(),
        width: target_w,
        height: target_h,
    }
}
