//! Metadata sidecar structs for captured frames (JSON sidecar files).
//!
//! The sidecar records the render backend provenance (wgpu adapter name,
//! backend type, driver version), resolution, frame number and capture time.
//! This is what makes reference frames attributable to a specific backend.

use bevy::render::renderer::RenderAdapterInfo;
use serde::Serialize;

use crate::depth::DepthMeta;

/// Render backend provenance, read from the main-world `RenderAdapterInfo`
/// resource (public Bevy API, populated from wgpu `AdapterInfo`).
#[derive(Serialize, Clone, Debug)]
pub struct AdapterMeta {
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    pub device_type: String,
    pub driver: String,
    pub driver_info: String,
    pub backend: String,
}

impl AdapterMeta {
    pub fn from_adapter_info(info: &RenderAdapterInfo) -> Self {
        Self {
            name: info.name.clone(),
            vendor: info.vendor,
            device: info.device,
            device_type: format!("{:#?}", info.device_type),
            driver: info.driver.clone(),
            driver_info: info.driver_info.clone(),
            backend: format!("{:#?}", info.backend),
        }
    }

    pub fn unavailable() -> Self {
        Self {
            name: "unknown (no RenderAdapterInfo resource)".into(),
            vendor: 0,
            device: 0,
            device_type: "unknown".into(),
            driver: "unknown".into(),
            driver_info: "unknown".into(),
            backend: "unknown".into(),
        }
    }
}

/// Determinism configuration for the capture.
#[derive(Serialize, Clone, Debug)]
pub struct DeterminismMeta {
    pub camera: String,
    pub time_source: String,
    pub animation_seed: String,
}

/// Per-frame JSON sidecar.
#[derive(Serialize, Clone, Debug)]
pub struct FrameMetadata {
    pub tool: String,
    pub tool_version: String,
    pub bevy_version: String,
    pub wgpu_version: String,
    pub platform: String,
    pub arch: String,
    pub scene: String,
    pub frame: u32,
    pub width: u32,
    pub height: u32,
    pub surface_logical_size: (f32, f32),
    pub scale_factor: f32,
    pub pixel_format: String,
    pub captured_at_unix_secs: u64,
    pub captured_at_utc: String,
    pub capture_source: String,
    /// True when every pixel of the captured image is identical (frames
    /// captured before render pipelines finished compiling). Such frames are
    /// not valid reference content; increase `--warmup`.
    pub image_uniform: bool,
    pub files: FrameFiles,
    /// Depth-buffer EXR metadata; `None` when depth capture is unavailable for
    /// this frame (see README §5).
    pub depth: Option<DepthMeta>,
    pub adapter: AdapterMeta,
    pub determinism: DeterminismMeta,
}

/// File names produced for this frame.
#[derive(Serialize, Clone, Debug)]
pub struct FrameFiles {
    pub png: String,
    pub exr: String,
    pub json: String,
    /// Depth-buffer EXR file name; `None` when depth capture is unavailable.
    pub depth: Option<String>,
}

pub fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// RFC3339 UTC timestamp without external dependencies (civil-from-days
/// algorithm, valid for years 1970..~2500).
pub fn unix_to_rfc3339_utc(unix_secs: u64) -> String {
    let days = unix_secs / 86_400;
    let secs_of_day = unix_secs % 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    let h = secs_of_day / 3600;
    let mi = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Howard Hinnant's `civil_from_days` (public domain).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_known_epoch() {
        assert_eq!(unix_to_rfc3339_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn rfc3339_known_date() {
        assert_eq!(unix_to_rfc3339_utc(1_752_710_400), "2025-07-17T00:00:00Z");
    }

    #[test]
    fn rfc3339_roundtrip_second() {
        assert_eq!(unix_to_rfc3339_utc(1_700_000_123), "2023-11-14T22:15:23Z");
    }
}
