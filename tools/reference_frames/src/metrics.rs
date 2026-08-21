//! Pure image comparison metrics: SSIM, PSNR, pixel-diff histogram.
//!
//! All functions operate on raw 8-bit RGB buffers of equal dimensions, so the
//! module has no image-format dependencies and is fully unit-testable.
//!
//! - SSIM: grayscale structural similarity, 8x8 sliding windows (uniform
//!   weights), standard luminance/contrast constants for L=255. Identical
//!   images yield exactly 1.0; visibly different images yield < 1.0.
//! - PSNR: 10*log10(255^2 / MSE) over all RGB channels; identical images
//!   yield +inf.
//! - Diff histogram: per-pixel absolute channel differences binned 0..=255,
//!   plus mean and max stats.

use serde::Serialize;

/// SSIM luminance constant (L = 255).
const C1: f64 = (0.01f64 * 255.0) * (0.01f64 * 255.0);
/// SSIM contrast constant (L = 255).
const C2: f64 = (0.03f64 * 255.0) * (0.03f64 * 255.0);
/// SSIM window size.
const WIN: usize = 8;

/// Comparison result between two same-sized RGB images.
#[derive(Serialize, Clone, Debug)]
pub struct DiffStats {
    pub width: u32,
    pub height: u32,
    /// Mean SSIM over all 8x8 windows and all RGB channels. In [0, 1], 1.0 = identical.
    pub ssim: f64,
    /// PSNR in dB over all RGB channels (+inf when images are identical).
    /// JSON has no infinity literal, so +inf is serialized as the string "inf".
    #[serde(serialize_with = "serialize_inf_f64")]
    pub psnr_db: f64,
    /// Mean squared error over all RGB channels (0.0 when identical).
    pub mse: f64,
    pub mean_abs_diff: f64,
    pub max_abs_diff: u8,
    /// Histogram of per-pixel absolute channel differences, bins 0..=255.
    pub diff_histogram: Vec<u64>,
    /// Sum of the histogram (equals width*height*3 when both buffers are valid).
    pub histogram_total: u64,
}

fn serialize_inf_f64<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.is_infinite() {
        s.serialize_str("inf")
    } else {
        s.serialize_f64(*v)
    }
}

/// Compares two RGB8 buffers of size `width x height`.
///
/// Returns `None` when buffers have unexpected lengths or the dimensions are
/// zero. SSIM is only defined when the image is at least 8x8; for smaller
/// images the SSIM component is reported as 1.0 (no windows to compare).
pub fn compare_rgb(a: &[u8], b: &[u8], width: u32, height: u32) -> Option<DiffStats> {
    let expected = (width as usize).checked_mul(height as usize)? * 3;
    if width == 0 || height == 0 || a.len() != expected || b.len() != expected {
        return None;
    }

    let mse = mse_rgb(a, b);
    let psnr_db = psnr_from_mse(mse);
    let (histogram, histogram_total, mean_abs_diff, max_abs_diff) = diff_stats(a, b);

    Some(DiffStats {
        width,
        height,
        ssim: ssim_rgb(a, b, width as usize, height as usize),
        psnr_db,
        mse,
        mean_abs_diff,
        max_abs_diff,
        diff_histogram: histogram.to_vec(),
        histogram_total,
    })
}

/// MSE over all RGB channels.
fn mse_rgb(a: &[u8], b: &[u8]) -> f64 {
    let n = a.len();
    let mut acc = 0u64;
    for i in 0..n {
        let d = (a[i] as i64 - b[i] as i64).unsigned_abs();
        acc += (d * d) as u64;
    }
    acc as f64 / n as f64
}

/// PSNR from MSE. +inf when MSE is 0 (identical images).
fn psnr_from_mse(mse: f64) -> f64 {
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64.powi(2) / mse).log10()
    }
}

/// Per-pixel absolute channel-difference histogram plus mean/max stats.
fn diff_stats(a: &[u8], b: &[u8]) -> ([u64; 256], u64, f64, u8) {
    let mut histogram = [0u64; 256];
    let mut total: u64 = 0;
    let mut sum: u64 = 0;
    let mut max: u8 = 0;
    for i in 0..a.len() {
        let d = (a[i] as i64 - b[i] as i64).unsigned_abs() as u8;
        histogram[d as usize] += 1;
        total += 1;
        sum += d as u64;
        max = max.max(d);
    }
    (histogram, total, sum as f64 / total.max(1) as f64, max)
}

/// Grayscale SSIM between two RGB buffers (average of per-channel SSIM).
/// Returns 1.0 when there are no valid windows (image smaller than 8x8).
fn ssim_rgb(a: &[u8], b: &[u8], width: usize, height: usize) -> f64 {
    if width < WIN || height < WIN {
        return 1.0;
    }
    let mut total = 0.0;
    let mut count = 0.0;
    for ch in 0..3 {
        let (val, n) = ssim_channel(a, b, width, height, ch);
        total += val;
        count += n as f64;
    }
    if count == 0.0 {
        1.0
    } else {
        total / count
    }
}

/// SSIM for a single channel with 8x8 sliding windows (uniform weights).
/// Returns (sum of window SSIM values, number of windows).
fn ssim_channel(a: &[u8], b: &[u8], width: usize, height: usize, ch: usize) -> (f64, usize) {
    let n_windows_x = width - WIN + 1;
    let n_windows_y = height - WIN + 1;
    let window_px = (WIN * WIN) as f64;
    let mut acc = 0.0f64;
    let mut count = 0usize;

    for wy in 0..n_windows_y {
        for wx in 0..n_windows_x {
            let mut mean_a = 0.0f64;
            let mut mean_b = 0.0f64;
            for j in 0..WIN {
                for i in 0..WIN {
                    let idx = (((wy + j) * width) + (wx + i)) * 3 + ch;
                    mean_a += a[idx] as f64;
                    mean_b += b[idx] as f64;
                }
            }
            mean_a /= window_px;
            mean_b /= window_px;

            let mut var_a = 0.0f64;
            let mut var_b = 0.0f64;
            let mut cov = 0.0f64;
            for j in 0..WIN {
                for i in 0..WIN {
                    let idx = (((wy + j) * width) + (wx + i)) * 3 + ch;
                    let da = a[idx] as f64 - mean_a;
                    let db = b[idx] as f64 - mean_b;
                    var_a += da * da;
                    var_b += db * db;
                    cov += da * db;
                }
            }
            var_a /= window_px;
            var_b /= window_px;
            cov /= window_px;

            let numerator = (2.0 * mean_a * mean_b + C1) * (2.0 * cov + C2);
            let denominator =
                (mean_a * mean_a + mean_b * mean_b + C1) * (var_a + var_b + C2);
            acc += numerator / denominator;
            count += 1;
        }
    }
    (acc, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 16x16 deterministic pseudo-random RGB pattern (fixed seed).
    fn pattern(seed: u64, w: u32, h: u32) -> Vec<u8> {
        let mut state = seed;
        let mut next = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as u8
        };
        (0..w as usize * h as usize * 3).map(|_| next()).collect()
    }

    #[test]
    fn identical_images_give_ssim_1_and_psnr_inf() {
        let a = pattern(42, 16, 16);
        let b = a.clone();
        let stats = compare_rgb(&a, &b, 16, 16).expect("valid buffers");
        assert!((stats.ssim - 1.0).abs() < 1e-9, "ssim was {}", stats.ssim);
        assert_eq!(stats.psnr_db, f64::INFINITY);
        assert_eq!(stats.mse, 0.0);
        assert_eq!(stats.max_abs_diff, 0);
        assert_eq!(stats.histogram_total, 16 * 16 * 3);
        assert_eq!(stats.diff_histogram[0], 16 * 16 * 3);
        assert!(stats.diff_histogram.iter().skip(1).all(|&c| c == 0));
    }

    #[test]
    fn constant_shift_gives_psnr_0_and_low_ssim() {
        let w = 16u32;
        let h = 16u32;
        let a = vec![0u8; (w * h * 3) as usize];
        let b = vec![255u8; (w * h * 3) as usize];
        let stats = compare_rgb(&a, &b, w, h).expect("valid buffers");
        assert_eq!(stats.mse, 255.0 * 255.0);
        assert!((stats.psnr_db - 0.0).abs() < 1e-9, "psnr was {}", stats.psnr_db);
        assert!(stats.ssim < 0.01, "ssim was {}", stats.ssim);
        assert_eq!(stats.max_abs_diff, 255);
        assert_eq!(stats.diff_histogram[255], (w * h * 3) as u64);
    }

    #[test]
    fn small_perturbation_is_detected() {
        let mut a = pattern(7, 32, 32);
        let b = a.clone();
        // Flip one pixel in channel 1.
        a[100] = a[100].wrapping_add(1);
        let stats = compare_rgb(&a, &b, 32, 32).expect("valid buffers");
        assert!(stats.ssim < 1.0, "ssim was {}", stats.ssim);
        assert!(stats.psnr_db.is_finite(), "psnr was {}", stats.psnr_db);
        assert!(stats.psnr_db > 0.0);
        assert_eq!(stats.max_abs_diff, 1);
        assert_eq!(stats.diff_histogram[1], 1);
        assert_eq!(stats.histogram_total, 32 * 32 * 3);
    }

    #[test]
    fn shifted_edges_reduce_ssim() {
        // Left half white / right half black, shifted by one pixel.
        let w = 64u32;
        let h = 64u32;
        let mut a = vec![255u8; (w * h * 3) as usize];
        let mut b = vec![255u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let v_a = if x < 32 { 0 } else { 255 };
                let v_b = if x < 31 { 0 } else { 255 };
                for ch in 0..3 {
                    a[(y * w + x) as usize * 3 + ch] = v_a;
                    b[(y * w + x) as usize * 3 + ch] = v_b;
                }
            }
        }
        let stats = compare_rgb(&a, &b, w, h).expect("valid buffers");
        assert!(stats.ssim < 1.0, "ssim was {}", stats.ssim);
        assert!(stats.ssim > 0.9, "ssim was {} (expected high but not 1.0)", stats.ssim);
        // 1 px wide column differs by 255 over 64 rows x 3 channels:
        // MSE = 255^2/64, PSNR = 10*log10(64) ≈ 18.06 dB.
        assert!(stats.psnr_db.is_finite());
        assert!(stats.psnr_db > 10.0, "psnr was {}", stats.psnr_db);
        assert_eq!(stats.diff_histogram[255], (h * 3) as u64);
    }

    #[test]
    fn mismatched_dimensions_return_none() {
        let a = pattern(1, 8, 8);
        let b = pattern(2, 8, 8);
        assert!(compare_rgb(&a, &b, 8, 7).is_none());
        assert!(compare_rgb(&a, &b, 8, 8).is_some());
        assert!(compare_rgb(&[], &b, 8, 8).is_none());
        assert!(compare_rgb(&a, &b, 0, 8).is_none());
    }

    #[test]
    fn tiny_images_do_not_panic() {
        let a = pattern(3, 4, 4);
        let b = pattern(4, 4, 4);
        let stats = compare_rgb(&a, &b, 4, 4).expect("valid buffers");
        // No 8x8 windows fit; SSIM is vacuously 1.0, PSNR still meaningful.
        assert_eq!(stats.ssim, 1.0);
        assert!(stats.psnr_db.is_finite());
    }

    #[test]
    fn non_square_window_count_matches_formula() {
        let a = pattern(5, 16, 9);
        let b = a.clone();
        let stats = compare_rgb(&a, &b, 16, 9).expect("valid buffers");
        assert_eq!(stats.ssim, 1.0);
        // windows = (16-8+1) * (9-8+1) = 9 * 2 = 18 per channel.
        let windows = ssim_channel(&a, &b, 16, 9, 0).1;
        assert_eq!(windows, 18);
    }

    #[test]
    fn psnr_inf_serializes_as_string() {
        let a = pattern(9, 8, 8);
        let stats = compare_rgb(&a, &a, 8, 8).expect("valid buffers");
        let json = serde_json::to_string(&stats).expect("serializable");
        assert!(json.contains("\"psnr_db\":\"inf\""), "json was: {json}");
    }
}
