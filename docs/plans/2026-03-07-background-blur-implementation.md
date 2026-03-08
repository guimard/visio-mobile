# Background Blur Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add real-time on-device background blur/replacement to the camera pipeline on all 3 platforms.

**Architecture:** A single Rust module in visio-ffi handles person segmentation (via ONNX Runtime + MediaPipe selfie segmentation model) and Gaussian blur compositing on I420 frames. The module intercepts frames after I420 conversion and before `capture_frame()` on all platforms. Settings are stored in visio-core. UI toggle is added to each platform's in-call settings.

**Tech Stack:** Rust, ort (ONNX Runtime crate), jpeg-decoder, MediaPipe selfie segmentation model (ONNX format, ~200KB), I420 pixel manipulation, 8 background images from suitenumerique/meet (~8.7MB total)

---

## Task 1: Add ONNX Runtime dependency and model management

**Files:**
- Modify: `crates/visio-ffi/Cargo.toml`
- Create: `crates/visio-ffi/src/blur/mod.rs`
- Create: `crates/visio-ffi/src/blur/model.rs`
- Create: `models/selfie_segmentation.onnx` (downloaded separately)

**Step 1: Add `ort` dependency to visio-ffi**

In `crates/visio-ffi/Cargo.toml`, add:
```toml
[dependencies]
ort = { version = "2", default-features = false, features = ["ndarray"] }
ndarray = "0.16"
```

**Step 2: Create blur module skeleton**

Create `crates/visio-ffi/src/blur/mod.rs`:
```rust
pub mod model;
mod process;

pub use process::BlurProcessor;
```

Create `crates/visio-ffi/src/blur/model.rs`:
```rust
use ort::session::Session;
use std::path::Path;
use std::sync::OnceLock;

static SESSION: OnceLock<Session> = OnceLock::new();

/// Load the selfie segmentation ONNX model from the given path.
/// Called once at app startup or first blur enable.
pub fn load_model(model_path: &Path) -> Result<(), String> {
    let session = Session::builder()
        .map_err(|e| format!("ort session builder: {e}"))?
        .with_intra_threads(2)
        .map_err(|e| format!("ort threads: {e}"))?
        .commit_from_file(model_path)
        .map_err(|e| format!("ort load model: {e}"))?;
    SESSION.set(session).map_err(|_| "model already loaded".into())
}

pub fn get_session() -> Option<&'static Session> {
    SESSION.get()
}
```

**Step 3: Download MediaPipe selfie segmentation model**

The ONNX model will be bundled with each platform's assets. Download from MediaPipe model zoo and convert to ONNX format. Place at `models/selfie_segmentation.onnx` for reference.

Note: The model takes 256x256 RGB input and outputs a 256x256 single-channel mask (0.0 = background, 1.0 = person).

**Step 4: Verify it compiles**

Run: `cargo build -p visio-ffi 2>&1 | tail -10`
Expected: Compiles (model not loaded yet, just structure)

**Step 5: Commit**

```
feat(ffi): add ONNX Runtime dependency and blur module skeleton
```

---

## Task 2: Implement I420 ↔ RGB conversion utilities

**Files:**
- Create: `crates/visio-ffi/src/blur/convert.rs`

**Step 1: Write I420-to-RGB and RGB-to-I420 conversion functions**

The segmentation model needs RGB input. Camera frames are I420. We need bidirectional conversion.

```rust
/// Convert I420 planes to packed RGB (BT.601 full-range).
/// Output: Vec<u8> of length width * height * 3.
pub fn i420_to_rgb(
    y: &[u8], u: &[u8], v: &[u8],
    width: usize, height: usize,
    stride_y: usize, stride_u: usize, stride_v: usize,
) -> Vec<u8> {
    let mut rgb = vec![0u8; width * height * 3];
    for row in 0..height {
        for col in 0..width {
            let y_val = y[row * stride_y + col] as f32;
            let u_val = u[(row / 2) * stride_u + col / 2] as f32 - 128.0;
            let v_val = v[(row / 2) * stride_v + col / 2] as f32 - 128.0;
            let r = (y_val + 1.402 * v_val).clamp(0.0, 255.0) as u8;
            let g = (y_val - 0.344 * u_val - 0.714 * v_val).clamp(0.0, 255.0) as u8;
            let b = (y_val + 1.772 * u_val).clamp(0.0, 255.0) as u8;
            let idx = (row * width + col) * 3;
            rgb[idx] = r;
            rgb[idx + 1] = g;
            rgb[idx + 2] = b;
        }
    }
    rgb
}

/// Resize RGB image to target dimensions using bilinear interpolation.
pub fn resize_rgb(
    src: &[u8], src_w: usize, src_h: usize,
    dst_w: usize, dst_h: usize,
) -> Vec<u8> {
    let mut dst = vec![0u8; dst_w * dst_h * 3];
    let x_ratio = src_w as f32 / dst_w as f32;
    let y_ratio = src_h as f32 / dst_h as f32;
    for y in 0..dst_h {
        for x in 0..dst_w {
            let src_x = x as f32 * x_ratio;
            let src_y = y as f32 * y_ratio;
            let x0 = src_x as usize;
            let y0 = src_y as usize;
            let x1 = (x0 + 1).min(src_w - 1);
            let y1 = (y0 + 1).min(src_h - 1);
            let fx = src_x - x0 as f32;
            let fy = src_y - y0 as f32;
            for c in 0..3 {
                let v00 = src[(y0 * src_w + x0) * 3 + c] as f32;
                let v10 = src[(y0 * src_w + x1) * 3 + c] as f32;
                let v01 = src[(y1 * src_w + x0) * 3 + c] as f32;
                let v11 = src[(y1 * src_w + x1) * 3 + c] as f32;
                let val = v00 * (1.0 - fx) * (1.0 - fy)
                    + v10 * fx * (1.0 - fy)
                    + v01 * (1.0 - fx) * fy
                    + v11 * fx * fy;
                dst[(y * dst_w + x) * 3 + c] = val.clamp(0.0, 255.0) as u8;
            }
        }
    }
    dst
}
```

**Step 2: Add JPEG decoding and RGB-to-I420 conversion (for background image replacement)**

Add `jpeg-decoder` dependency to `crates/visio-ffi/Cargo.toml`:
```toml
jpeg-decoder = "0.3"
```

```rust
/// Decode JPEG bytes to packed RGB.
pub fn decode_jpeg_to_rgb(jpeg_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = jpeg_decoder::Decoder::new(jpeg_bytes);
    let pixels = decoder.decode().map_err(|e| e.to_string())?;
    Ok(pixels)
}

/// Get JPEG dimensions without fully decoding.
pub fn jpeg_dimensions(jpeg_bytes: &[u8]) -> Result<(usize, usize), String> {
    let mut decoder = jpeg_decoder::Decoder::new(jpeg_bytes);
    decoder.read_info().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("no JPEG info")?;
    Ok((info.width as usize, info.height as usize))
}

/// Convert packed RGB to I420 planes (BT.601 full-range).
pub fn rgb_to_i420(rgb: &[u8], width: usize, height: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; width * height];
    let uv_w = width / 2;
    let uv_h = height / 2;
    let mut u_plane = vec![0u8; uv_w * uv_h];
    let mut v_plane = vec![0u8; uv_w * uv_h];
    for row in 0..height {
        for col in 0..width {
            let idx = (row * width + col) * 3;
            let r = rgb[idx] as f32;
            let g = rgb[idx + 1] as f32;
            let b = rgb[idx + 2] as f32;
            y[row * width + col] = (0.299 * r + 0.587 * g + 0.114 * b).clamp(0.0, 255.0) as u8;
            if row % 2 == 0 && col % 2 == 0 {
                let uv_idx = (row / 2) * uv_w + col / 2;
                u_plane[uv_idx] = (-0.169 * r - 0.331 * g + 0.500 * b + 128.0).clamp(0.0, 255.0) as u8;
                v_plane[uv_idx] = (0.500 * r - 0.419 * g - 0.081 * b + 128.0).clamp(0.0, 255.0) as u8;
            }
        }
    }
    (y, u_plane, v_plane)
}
```

**Step 3: Add unit tests for conversions**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i420_to_rgb_black_frame() {
        // Y=0, U=128, V=128 → RGB(0, 0, 0)
        let y = vec![0u8; 4];
        let u = vec![128u8; 1];
        let v = vec![128u8; 1];
        let rgb = i420_to_rgb(&y, &u, &v, 2, 2, 2, 1, 1);
        assert!(rgb.iter().all(|&b| b == 0));
    }

    #[test]
    fn i420_to_rgb_white_frame() {
        // Y=255, U=128, V=128 → RGB(255, 255, 255)
        let y = vec![255u8; 4];
        let u = vec![128u8; 1];
        let v = vec![128u8; 1];
        let rgb = i420_to_rgb(&y, &u, &v, 2, 2, 2, 1, 1);
        assert!(rgb.iter().all(|&b| b == 255));
    }

    #[test]
    fn resize_rgb_identity() {
        let src = vec![100u8; 2 * 2 * 3];
        let dst = resize_rgb(&src, 2, 2, 2, 2);
        assert_eq!(src, dst);
    }

    #[test]
    fn resize_rgb_downsample() {
        let src = vec![200u8; 4 * 4 * 3];
        let dst = resize_rgb(&src, 4, 4, 2, 2);
        assert_eq!(dst.len(), 2 * 2 * 3);
        assert!(dst.iter().all(|&b| b == 200));
    }
}
```

**Step 3: Run tests**

Run: `cargo test -p visio-ffi --lib blur 2>&1 | tail -15`
Expected: All tests pass

**Step 4: Commit**

```
feat(ffi): add I420-RGB conversion utilities with tests
```

---

## Task 3: Implement segmentation inference

**Files:**
- Create: `crates/visio-ffi/src/blur/segment.rs`

**Step 1: Implement the segmentation function**

```rust
use ndarray::{Array, CowArray, IxDyn};
use ort::session::Session;

/// Run selfie segmentation on an RGB image.
/// Input: 256x256 RGB image (packed u8).
/// Output: 256x256 mask (f32, 0.0=background, 1.0=person).
pub fn segment(session: &Session, rgb_256: &[u8]) -> Result<Vec<f32>, String> {
    assert_eq!(rgb_256.len(), 256 * 256 * 3);

    // Normalize to [0, 1] and reshape to NCHW: [1, 3, 256, 256]
    let mut input = vec![0.0f32; 1 * 3 * 256 * 256];
    for i in 0..(256 * 256) {
        input[i] = rgb_256[i * 3] as f32 / 255.0;              // R
        input[256 * 256 + i] = rgb_256[i * 3 + 1] as f32 / 255.0; // G
        input[2 * 256 * 256 + i] = rgb_256[i * 3 + 2] as f32 / 255.0; // B
    }

    let input_array = CowArray::from(
        Array::from_shape_vec(IxDyn(&[1, 3, 256, 256]), input)
            .map_err(|e| format!("input shape: {e}"))?
    );

    let outputs = session
        .run(ort::inputs![input_array].map_err(|e| format!("ort inputs: {e}"))?)
        .map_err(|e| format!("ort run: {e}"))?;

    let output_tensor = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| format!("ort extract: {e}"))?;

    // Output is [1, 1, 256, 256] or [1, 256, 256] — flatten to 256*256
    let mask: Vec<f32> = output_tensor.iter().copied().collect();
    Ok(mask)
}

/// Resize a 256x256 f32 mask to target dimensions using bilinear interpolation.
pub fn resize_mask(mask: &[f32], dst_w: usize, dst_h: usize) -> Vec<f32> {
    let src_w = 256;
    let src_h = 256;
    let mut dst = vec![0.0f32; dst_w * dst_h];
    let x_ratio = src_w as f32 / dst_w as f32;
    let y_ratio = src_h as f32 / dst_h as f32;
    for y in 0..dst_h {
        for x in 0..dst_w {
            let src_x = x as f32 * x_ratio;
            let src_y = y as f32 * y_ratio;
            let x0 = src_x as usize;
            let y0 = src_y as usize;
            let x1 = (x0 + 1).min(src_w - 1);
            let y1 = (y0 + 1).min(src_h - 1);
            let fx = src_x - x0 as f32;
            let fy = src_y - y0 as f32;
            dst[y * dst_w + x] = mask[y0 * src_w + x0] * (1.0 - fx) * (1.0 - fy)
                + mask[y0 * src_w + x1] * fx * (1.0 - fy)
                + mask[y1 * src_w + x0] * (1.0 - fx) * fy
                + mask[y1 * src_w + x1] * fx * fy;
        }
    }
    dst
}
```

**Step 2: Commit**

```
feat(ffi): add ONNX-based selfie segmentation inference
```

---

## Task 4: Implement Gaussian blur on I420

**Files:**
- Create: `crates/visio-ffi/src/blur/gaussian.rs`

**Step 1: Implement box blur approximation on Y/U/V planes**

A 3-pass box blur approximates Gaussian blur and is much faster (O(n) per pass, independent of radius).

```rust
/// Apply 3-pass box blur approximation of Gaussian blur on a single plane.
/// `data`: pixel values, `width`/`height`: plane dimensions, `stride`: row stride.
/// `radius`: blur radius in pixels.
/// Returns a new buffer with the blurred plane.
pub fn blur_plane(
    data: &[u8], width: usize, height: usize, stride: usize, radius: usize,
) -> Vec<u8> {
    let mut src = extract_plane(data, width, height, stride);
    let mut dst = vec![0u8; width * height];
    // 3-pass box blur
    for _ in 0..3 {
        box_blur_h(&src, &mut dst, width, height, radius);
        box_blur_v(&dst, &mut src, width, height, radius);
    }
    src
}

fn extract_plane(data: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height];
    for row in 0..height {
        out[row * width..(row + 1) * width]
            .copy_from_slice(&data[row * stride..row * stride + width]);
    }
    out
}

fn box_blur_h(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let diameter = 2 * r + 1;
    for y in 0..h {
        let mut sum = 0u32;
        // Initialize window
        for x in 0..=r.min(w - 1) {
            sum += src[y * w + x] as u32;
        }
        // Left edge padding
        sum += r.saturating_sub(0) as u32 * src[y * w] as u32;

        for x in 0..w {
            dst[y * w + x] = (sum / diameter as u32).min(255) as u8;
            let right = (x + r + 1).min(w - 1);
            let left = (x as isize - r as isize).max(0) as usize;
            sum += src[y * w + right] as u32;
            sum -= src[y * w + left] as u32;
        }
    }
}

fn box_blur_v(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let diameter = 2 * r + 1;
    for x in 0..w {
        let mut sum = 0u32;
        for y in 0..=r.min(h - 1) {
            sum += src[y * w + x] as u32;
        }
        sum += r.saturating_sub(0) as u32 * src[x] as u32;

        for y in 0..h {
            dst[y * w + x] = (sum / diameter as u32).min(255) as u8;
            let bottom = (y + r + 1).min(h - 1);
            let top = (y as isize - r as isize).max(0) as usize;
            sum += src[bottom * w + x] as u32;
            sum -= src[top * w + x] as u32;
        }
    }
}
```

**Step 2: Add tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_uniform_plane_unchanged() {
        let data = vec![128u8; 10 * 10];
        let result = blur_plane(&data, 10, 10, 10, 3);
        // Uniform input → uniform output
        for &v in &result {
            assert!((v as i16 - 128).abs() <= 1);
        }
    }

    #[test]
    fn blur_reduces_contrast() {
        let mut data = vec![0u8; 10 * 10];
        // White center pixel
        data[5 * 10 + 5] = 255;
        let result = blur_plane(&data, 10, 10, 10, 2);
        // Center should be dimmer, neighbors should be brighter
        assert!(result[5 * 10 + 5] < 255);
        assert!(result[5 * 10 + 4] > 0);
    }
}
```

**Step 3: Run tests**

Run: `cargo test -p visio-ffi --lib blur 2>&1 | tail -15`

**Step 4: Commit**

```
feat(ffi): add fast box blur approximation for I420 planes
```

---

## Task 5: Implement the BlurProcessor (compositing)

**Files:**
- Create: `crates/visio-ffi/src/blur/process.rs`

**Step 1: Implement the main processor that ties everything together**

```rust
use super::{convert, gaussian, model, segment};
use std::sync::atomic::{AtomicBool, Ordering};

use std::sync::Mutex;

/// Background mode: Off, Blur, or Image replacement (by image ID 1-8).
#[derive(Clone, Debug, PartialEq)]
pub enum BackgroundMode {
    Off,
    Blur,
    Image(u8), // 1-8, corresponds to assets/backgrounds/{id}.jpg
}

static MODE: Mutex<BackgroundMode> = Mutex::new(BackgroundMode::Off);

/// Cached replacement image in I420 format, resized to current frame dimensions.
static REPLACEMENT_CACHE: Mutex<Option<ReplacementImage>> = Mutex::new(None);

struct ReplacementImage {
    id: u8,
    width: usize,
    height: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

/// Blur radius for background (applied to each I420 plane scaled appropriately).
const Y_BLUR_RADIUS: usize = 15;
const UV_BLUR_RADIUS: usize = 7; // Half resolution

pub struct BlurProcessor;

impl BlurProcessor {
    pub fn set_mode(mode: BackgroundMode) {
        // Clear replacement cache if mode changes
        {
            let mut cache = REPLACEMENT_CACHE.lock().unwrap();
            if let BackgroundMode::Image(id) = &mode {
                if let Some(ref c) = *cache {
                    if c.id != *id { *cache = None; }
                }
            } else {
                *cache = None;
            }
        }
        *MODE.lock().unwrap() = mode;
    }

    pub fn get_mode() -> BackgroundMode {
        MODE.lock().unwrap().clone()
    }

    /// Load a replacement image from raw JPEG bytes, decode to RGB, convert to I420.
    /// Called once per image change (not per frame).
    pub fn load_replacement_image(id: u8, jpeg_bytes: &[u8], target_w: usize, target_h: usize) -> Result<(), String> {
        // Decode JPEG → RGB → resize to target → convert to I420
        let rgb = convert::decode_jpeg_to_rgb(jpeg_bytes)?;
        let (img_w, img_h) = convert::jpeg_dimensions(jpeg_bytes)?;
        let rgb_resized = convert::resize_rgb(&rgb, img_w, img_h, target_w, target_h);
        let (y, u, v) = convert::rgb_to_i420(&rgb_resized, target_w, target_h);
        let mut cache = REPLACEMENT_CACHE.lock().unwrap();
        *cache = Some(ReplacementImage { id, width: target_w, height: target_h, y, u, v });
        Ok(())
    }

    /// Process an I420 frame: segment person, blur/replace background, composite.
    /// Modifies the planes in-place.
    /// Returns false if processing could not be applied (model not loaded, mode Off, etc.).
    pub fn process_i420(
        y: &mut [u8], u: &mut [u8], v: &mut [u8],
        width: usize, height: usize,
        stride_y: usize, stride_u: usize, stride_v: usize,
    ) -> bool {
        let mode = Self::get_mode();
        if mode == BackgroundMode::Off {
            return false;
        }

        let session = match model::get_session() {
            Some(s) => s,
            None => return false,
        };

        // 1. Convert I420 to RGB for segmentation
        let rgb = convert::i420_to_rgb(y, u, v, width, height, stride_y, stride_u, stride_v);

        // 2. Resize to 256x256 for model
        let rgb_256 = convert::resize_rgb(&rgb, width, height, 256, 256);

        // 3. Run segmentation
        let mask_256 = match segment::segment(session, &rgb_256) {
            Ok(m) => m,
            Err(_) => return false,
        };

        // 4. Resize mask back to frame dimensions
        let mask = segment::resize_mask(&mask_256, width, height);

        // 5. Get background planes (blurred or replacement image)
        let uv_w = width / 2;
        let uv_h = height / 2;
        let (bg_y, bg_u, bg_v) = match &mode {
            BackgroundMode::Blur => {
                let by = gaussian::blur_plane(y, width, height, stride_y, Y_BLUR_RADIUS);
                let bu = gaussian::blur_plane(u, uv_w, uv_h, stride_u, UV_BLUR_RADIUS);
                let bv = gaussian::blur_plane(v, uv_w, uv_h, stride_v, UV_BLUR_RADIUS);
                (by, bu, bv)
            }
            BackgroundMode::Image(_) => {
                let cache = REPLACEMENT_CACHE.lock().unwrap();
                match &*cache {
                    Some(img) if img.width == width && img.height == height => {
                        (img.y.clone(), img.u.clone(), img.v.clone())
                    }
                    _ => return false, // Image not loaded or wrong size
                }
            }
            BackgroundMode::Off => unreachable!(),
        };

        // 6. Composite: foreground (original) where mask > 0.5, background elsewhere
        for row in 0..height {
            for col in 0..width {
                let m = mask[row * width + col];
                let idx = row * stride_y + col;
                y[idx] = lerp_u8(bg_y[row * width + col], y[idx], m);
            }
        }
        for row in 0..uv_h {
            for col in 0..uv_w {
                // Average mask over the 2x2 block this chroma pixel covers
                let m = (mask[row * 2 * width + col * 2]
                    + mask[row * 2 * width + col * 2 + 1]
                    + mask[(row * 2 + 1) * width + col * 2]
                    + mask[(row * 2 + 1) * width + col * 2 + 1])
                    / 4.0;
                let idx_u = row * stride_u + col;
                let idx_v = row * stride_v + col;
                u[idx_u] = lerp_u8(bg_u[row * uv_w + col], u[idx_u], m);
                v[idx_v] = lerp_u8(bg_v[row * uv_w + col], v[idx_v], m);
            }
        }

        true
    }
}

#[inline]
fn lerp_u8(bg: u8, fg: u8, mask: f32) -> u8 {
    let m = mask.clamp(0.0, 1.0);
    (bg as f32 * (1.0 - m) + fg as f32 * m) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_extremes() {
        assert_eq!(lerp_u8(0, 255, 1.0), 255); // full foreground
        assert_eq!(lerp_u8(0, 255, 0.0), 0);   // full background
    }

    #[test]
    fn lerp_midpoint() {
        let result = lerp_u8(0, 200, 0.5);
        assert!((result as i16 - 100).abs() <= 1);
    }

    #[test]
    fn mode_default_is_off() {
        // Reset for test
        *MODE.lock().unwrap() = BackgroundMode::Off;
        assert_eq!(BlurProcessor::get_mode(), BackgroundMode::Off);
    }

    #[test]
    fn set_mode_roundtrip() {
        BlurProcessor::set_mode(BackgroundMode::Blur);
        assert_eq!(BlurProcessor::get_mode(), BackgroundMode::Blur);
        BlurProcessor::set_mode(BackgroundMode::Image(3));
        assert_eq!(BlurProcessor::get_mode(), BackgroundMode::Image(3));
        BlurProcessor::set_mode(BackgroundMode::Off);
        assert_eq!(BlurProcessor::get_mode(), BackgroundMode::Off);
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p visio-ffi --lib blur 2>&1 | tail -15`

**Step 3: Commit**

```
feat(ffi): add BlurProcessor compositing pipeline
```

---

## Task 6: Add background mode setting to visio-core

**Files:**
- Modify: `crates/visio-core/src/settings.rs`
- Modify: `crates/visio-core/src/controls.rs`

**Step 1: Add `BackgroundMode` to Settings struct**

In `settings.rs`, add a serializable background mode to the settings struct. Store as a string: `"off"`, `"blur"`, or `"image:N"` (N = 1-8). Default: `"off"`. Add `set_background_mode()` and `get_background_mode()` methods following the existing pattern.

**Step 2: Add background mode control to MeetingControls**

In `controls.rs`, add `set_background_mode(mode: &str)` and `get_background_mode() -> String` methods. These should call `BlurProcessor::set_mode()` when toggled, mapping the string to the `BackgroundMode` enum.

**Step 3: Add tests**

Follow the existing pattern in `settings::tests`:
```rust
#[test]
fn test_background_mode_defaults_to_off() {
    // ...
}

#[test]
fn test_set_background_mode_blur() {
    // ...
}

#[test]
fn test_set_background_mode_image() {
    // ...set to "image:3", read back, verify
}
```

**Step 4: Run tests**

Run: `cargo test -p visio-core --lib 2>&1 | tail -10`

**Step 5: Commit**

```
feat(core): add background_mode setting and control
```

---

## Task 7: Expose blur via UniFFI and hook into camera pipeline

**Files:**
- Modify: `crates/visio-ffi/src/lib.rs`

**Step 1: Add UniFFI methods**

Add `set_background_mode(mode: String)` and `get_background_mode() -> String` to the VisioClient FFI interface, delegating to `MeetingControls`. Also add `load_background_image(id: u8, jpeg_path: String)` which reads the JPEG file and calls `BlurProcessor::load_replacement_image()`.

**Step 2: Hook into Android camera path**

In `Java_io_visio_mobile_NativeVideo_nativePushCameraFrame()` (around line 890, after I420 construction, before `capture_frame()`):

```rust
// Apply background blur if enabled
blur::BlurProcessor::process_i420(
    &mut y_data, &mut u_data, &mut v_data,
    width as usize, height as usize,
    stride_y as usize, stride_u as usize, stride_v as usize,
);
// Then create VideoFrame from (possibly modified) data and call capture_frame
```

**Step 3: Hook into iOS camera path**

In `visio_push_ios_camera_frame()` (around line 1145, same pattern).

**Step 4: Verify it compiles**

Run: `cargo build -p visio-ffi 2>&1 | tail -10`

**Step 5: Commit**

```
feat(ffi): expose blur setting and hook into camera pipeline
```

---

## Task 8: Hook into Desktop camera path

**Files:**
- Modify: `crates/visio-desktop/src/camera_macos.rs`
- Modify: `crates/visio-desktop/src/lib.rs`

**Step 1: Add blur processing to desktop camera capture**

In `camera_macos.rs`, after I420 buffer construction (around line 145), call `BlurProcessor::process_i420()`.

**Step 2: Add Tauri commands for background mode**

In `lib.rs`, add commands:
```rust
#[tauri::command]
fn set_background_mode(state: State<'_, VisioState>, mode: String) -> Result<(), String> {
    // Parse mode string ("off", "blur", "image:N") and call BlurProcessor::set_mode()
    // If image mode, load the image from bundled resources
}

#[tauri::command]
fn get_background_mode(state: State<'_, VisioState>) -> String {
    // ...
}
```

**Step 3: Commit**

```
feat(desktop): integrate background blur into camera pipeline
```

---

## Task 9: Android UI — background mode picker

**Files:**
- Modify: `android/app/src/main/kotlin/io/visio/mobile/ui/InCallSettingsSheet.kt`
- Modify: `i18n/en.json`
- Modify: `i18n/fr.json`

**Step 1: Add background section to camera tab**

In the camera tab of InCallSettingsSheet, add a "Background" section with:
- "None" option (BackgroundMode.Off)
- "Blur" option with blur icon
- Grid of 8 thumbnail images (loaded from `assets/backgrounds/thumbnails/`)
- Selected item highlighted with primary color border

```kotlin
// Background mode section
Text(Strings.t("settings.incall.background", lang), style = MaterialTheme.typography.titleSmall)

// Off + Blur row
Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
    BackgroundOption(
        label = Strings.t("settings.incall.bgOff", lang),
        icon = Icons.Default.Block,
        selected = backgroundMode == "off",
        onClick = { setBackgroundMode("off") }
    )
    BackgroundOption(
        label = Strings.t("settings.incall.bgBlur", lang),
        icon = Icons.Default.BlurOn,
        selected = backgroundMode == "blur",
        onClick = { setBackgroundMode("blur") }
    )
}

// Image grid (4 columns, 2 rows)
LazyVerticalGrid(columns = GridCells.Fixed(4), ...) {
    items(8) { index ->
        val id = index + 1
        BackgroundImageOption(
            thumbnailAsset = "backgrounds/thumbnails/$id.jpg",
            selected = backgroundMode == "image:$id",
            onClick = { setBackgroundMode("image:$id") }
        )
    }
}
```

**Step 2: Add i18n keys**

en.json:
```json
"settings.incall.background": "Background",
"settings.incall.bgOff": "None",
"settings.incall.bgBlur": "Blur"
```

fr.json:
```json
"settings.incall.background": "Arrière-plan",
"settings.incall.bgOff": "Aucun",
"settings.incall.bgBlur": "Flou"
```

**Step 3: Copy background assets**

Copy `assets/backgrounds/` (8 full images + 8 thumbnails) to `android/app/src/main/assets/backgrounds/`.

**Step 4: Build and verify**

Run: `cd android && ./gradlew compileDebugKotlin 2>&1 | tail -10`

**Step 5: Commit**

```
feat(android): add background mode picker in in-call settings
```

---

## Task 10: iOS UI — background mode picker

**Files:**
- Modify: `ios/VisioMobile/Views/InCallSettingsSheet.swift`
- Modify: `ios/VisioMobile/VisioManager.swift`

**Step 1: Add `backgroundMode` published property to VisioManager**

In `VisioManager.swift`, add `@Published var backgroundMode: String = "off"`.

**Step 2: Add background section to camera tab**

In the camera section of InCallSettingsSheet, add a "Background" section with:
- "None" button (mode = "off")
- "Blur" button with icon
- Grid of 8 thumbnail images (bundled in app, loaded from `backgrounds/thumbnails/`)
- Selected item highlighted with primary color border

```swift
Section(Strings.t("settings.incall.background", lang: lang)) {
    // Off + Blur options
    HStack(spacing: 12) {
        BackgroundOptionButton(
            label: Strings.t("settings.incall.bgOff", lang: lang),
            systemIcon: "circle.slash",
            selected: manager.backgroundMode == "off",
            isDark: isDark
        ) { setMode("off") }

        BackgroundOptionButton(
            label: Strings.t("settings.incall.bgBlur", lang: lang),
            systemIcon: "aqi.medium",
            selected: manager.backgroundMode == "blur",
            isDark: isDark
        ) { setMode("blur") }
    }

    // Image grid (4 columns)
    LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 8), count: 4), spacing: 8) {
        ForEach(1...8, id: \.self) { id in
            if let img = UIImage(named: "backgrounds/thumbnails/\(id)") {
                Image(uiImage: img)
                    .resizable()
                    .aspectRatio(16/9, contentMode: .fill)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .overlay(
                        RoundedRectangle(cornerRadius: 8)
                            .stroke(manager.backgroundMode == "image:\(id)" ? VisioColors.primary500 : .clear, lineWidth: 3)
                    )
                    .onTapGesture { setMode("image:\(id)") }
            }
        }
    }
}
```

**Step 3: Add background images to Xcode project**

Copy `assets/backgrounds/` to `ios/VisioMobile/Resources/backgrounds/` and add to Xcode bundle resources.

**Step 4: Commit**

```
feat(ios): add background mode picker in in-call settings
```

---

## Task 11: Desktop UI — background mode picker

**Files:**
- Modify: `crates/visio-desktop/frontend/src/App.tsx`

**Step 1: Add background mode picker to settings/camera section**

Add a section with Off/Blur buttons and a grid of 8 thumbnail images:
```tsx
<div className="background-picker">
  <h4>{t("settings.incall.background")}</h4>
  <div className="bg-options">
    <button
      className={`bg-option ${mode === "off" ? "selected" : ""}`}
      onClick={() => setMode("off")}
    >
      {t("settings.incall.bgOff")}
    </button>
    <button
      className={`bg-option ${mode === "blur" ? "selected" : ""}`}
      onClick={() => setMode("blur")}
    >
      {t("settings.incall.bgBlur")}
    </button>
  </div>
  <div className="bg-grid">
    {[1,2,3,4,5,6,7,8].map(id => (
      <img
        key={id}
        src={`/backgrounds/thumbnails/${id}.jpg`}
        className={`bg-thumb ${mode === `image:${id}` ? "selected" : ""}`}
        onClick={() => setMode(`image:${id}`)}
      />
    ))}
  </div>
</div>
```

**Step 2: Copy background assets to Tauri resources**

Copy `assets/backgrounds/` to `crates/visio-desktop/frontend/public/backgrounds/`.

**Step 3: Commit**

```
feat(desktop): add background mode picker in settings
```

---

## Task 12: Model and image bundling

**Files:**
- Create: `scripts/download-models.sh`
- Modify: Android `build.gradle` (copy model + images to assets)
- Modify: iOS Xcode project (add model + images to bundle)

**Step 1: Create download script for ONNX model**

```bash
#!/bin/bash
# Download selfie segmentation ONNX model
MODEL_DIR="models"
mkdir -p "$MODEL_DIR"
# URL TBD — MediaPipe model zoo or self-hosted
curl -L -o "$MODEL_DIR/selfie_segmentation.onnx" "$MODEL_URL"
```

**Step 2: Platform-specific bundling**

Assets to bundle per platform:
- `models/selfie_segmentation.onnx` (~200KB)
- `assets/backgrounds/1-8.jpg` (8 full images, ~8.7MB total)
- `assets/backgrounds/thumbnails/1-8.jpg` (8 thumbnails, ~165KB total)

Bundling:
- Android: copy to `android/app/src/main/assets/` (models/ + backgrounds/)
- iOS: add to Xcode project as bundle resources in `Resources/`
- Desktop: include in Tauri resources directory

**Step 3: Load model on first background mode change**

In each platform, when mode is first changed from "off", call `model::load_model(path)` with the correct asset path.

**Step 4: Load replacement image on image mode selection**

When user selects `image:N`, platform reads the JPEG file bytes from assets and calls `BlurProcessor::load_replacement_image(id, jpeg_bytes, frame_width, frame_height)`.

**Step 5: Commit**

```
feat: bundle ONNX model and background images for all platforms
```

---

## Summary

| Task | Component | Est. complexity |
|------|-----------|-----------------|
| 1 | ONNX Runtime + module skeleton | Low |
| 2 | I420 ↔ RGB conversion | Medium |
| 3 | Segmentation inference | Medium |
| 4 | Gaussian blur (box blur approx) | Medium |
| 5 | BlurProcessor compositing | Medium |
| 6 | Background mode setting in visio-core | Low |
| 7 | FFI exposure + camera hooks (Android/iOS) | High |
| 8 | Desktop camera hook + Tauri commands | Medium |
| 9 | Android UI — background mode picker | Medium |
| 10 | iOS UI — background mode picker | Medium |
| 11 | Desktop UI — background mode picker | Medium |
| 12 | Model + image bundling | Medium |

**Total: 12 tasks. Branch: `feat/background-blur`. PR when all tasks pass.**
