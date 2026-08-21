//! Whitelist JSON loading and verdict evaluation for backend-pair comparisons.
//!
//! Schema: `tests/reference-frames/whitelist/README.md` §3 (v1.0).
//! A whitelist file encodes per-scene D3D12-tier relaxed thresholds plus
//! documented per-frame expected differences. The verdict maps a `DiffStats`
//! onto the whitelist and reports pass/fail per metric.
#![expect(dead_code, reason = "schema metadata fields are retained for full JSON round-trip")]

use serde::{Deserialize, Serialize};

use crate::metrics::DiffStats;

/// The whitelist document (schema_version "1.0").
#[derive(Deserialize, Clone, Debug)]
pub struct Whitelist {
    pub schema_version: String,
    pub whitelist_id: String,
    pub scene: String,
    pub tier: String,
    pub platform_pair: PlatformPair,
    pub thresholds: Thresholds,
    #[serde(default)]
    pub categories: Vec<Category>,
    pub notes: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct PlatformPair {
    pub baseline: EngineSide,
    pub candidate: EngineSide,
}

#[derive(Deserialize, Clone, Debug)]
pub struct EngineSide {
    pub engine: String,
    pub api: String,
    pub role: String,
}

/// Top-level relaxed thresholds. `null` means "not yet calibrated" (skip).
#[derive(Deserialize, Clone, Debug, Default)]
pub struct Thresholds {
    pub ssim: Option<f64>,
    pub psnr_db: Option<f64>,
    pub mean_abs_diff: Option<f64>,
    pub max_abs_diff: Option<f64>,
    pub diff_histogram_p95: Option<f64>,
    pub notes: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Category {
    pub id: String,
    pub label: String,
    pub description: String,
    pub policy: String,
    #[serde(default)]
    pub expected_differences: Vec<ExpectedDifference>,
    pub notes: Option<String>,
}

/// One documented difference entry (§3.4). `observed` may be the string "inf".
#[derive(Deserialize, Clone, Debug)]
pub struct ExpectedDifference {
    pub frame: String,
    pub metric: String,
    pub observed: MetricValue,
    pub threshold: f64,
    #[serde(default)]
    pub scope: Option<Value>,
    pub rationale: String,
}

/// Metric value that may be `inf` (serialized as the JSON string "inf").
#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum MetricValue {
    Number(f64),
    Inf(String),
}

impl MetricValue {
    pub fn as_f64(&self) -> f64 {
        match self {
            MetricValue::Number(v) => *v,
            MetricValue::Inf(_) => f64::INFINITY,
        }
    }
}

/// Scope: "whole_frame" or a JSON object `{ "region": [x0,y0,x1,y1] }`.
#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum Value {
    WholeFrame(String),
    Region { region: [u32; 4] },
}

/// Verdict of a single metric check.
#[derive(Serialize, Clone, Debug)]
pub struct MetricCheck {
    pub metric: String,
    pub observed: MetricObserved,
    pub threshold: f64,
    pub passed: bool,
}

/// Observed metric value (mirrors `DiffStats` field names).
#[derive(Serialize, Clone, Debug)]
#[serde(untagged)]
pub enum MetricObserved {
    F64(f64),
    String(String),
}

/// Per-metric checks plus overall pass/fail.
#[derive(Serialize, Clone, Debug)]
pub struct WhitelistVerdict {
    pub whitelist_id: String,
    pub scene: String,
    pub checks: Vec<MetricCheck>,
    pub passed: bool,
}

/// Builds the checks for a `DiffStats` against a whitelist's thresholds.
///
/// Rules:
/// - A `None` threshold is not yet calibrated and is skipped (never fails).
/// - Per-frame `expected_differences` entries whose `frame` equals `frame`
///   override the global threshold for that metric, when present.
/// - `psnr_db` is an upper-is-better metric; the rest are lower-is-better.
/// - The histogram p95 is derived from the cumulative diff histogram
///   (the 95th percentile bin value; 0 when the image is identical).
pub fn judge(stats: &DiffStats, whitelist: &Whitelist, frame: &str) -> WhitelistVerdict {
    let mut checks = Vec::new();

    // Global thresholds (skip None).
    let thresholds = [
        ("ssim", whitelist.thresholds.ssim, Some(metric_ssim(stats))),
        ("psnr_db", whitelist.thresholds.psnr_db, Some(metric_psnr(stats))),
        ("mean_abs_diff", whitelist.thresholds.mean_abs_diff, Some(stats.mean_abs_diff)),
        ("max_abs_diff", whitelist.thresholds.max_abs_diff, Some(stats.max_abs_diff as f64)),
        (
            "diff_histogram_p95",
            whitelist.thresholds.diff_histogram_p95,
            Some(histogram_p95(stats)),
        ),
    ];

    for (name, global_threshold, observed) in thresholds {
        // Per-frame override (first matching entry wins).
        let override_threshold = whitelist
            .categories
            .iter()
            .flat_map(|c| c.expected_differences.iter())
            .find(|d| d.metric == name && d.frame == frame)
            .map(|d| d.threshold);

        let threshold = override_threshold.or(global_threshold);
        let Some(threshold) = threshold else {
            continue;
        };
        let Some(observed) = observed else {
            continue;
        };

        let passed = match name {
            // Higher is better.
            "ssim" | "psnr_db" => observed >= threshold,
            _ => observed <= threshold,
        };
        checks.push(MetricCheck {
            metric: name.to_string(),
            observed: MetricObserved::F64(observed),
            threshold,
            passed,
        });
    }

    let passed = checks.iter().all(|c| c.passed);
    WhitelistVerdict {
        whitelist_id: whitelist.whitelist_id.clone(),
        scene: whitelist.scene.clone(),
        checks,
        passed,
    }
}

fn metric_ssim(stats: &DiffStats) -> f64 {
    stats.ssim
}

fn metric_psnr(stats: &DiffStats) -> f64 {
    stats.psnr_db
}

/// 95th percentile of the per-pixel channel-difference histogram.
/// Returns the smallest bin index whose cumulative count reaches 95% of the
/// total channel-difference count (0 when identical).
fn histogram_p95(stats: &DiffStats) -> f64 {
    let total = stats.histogram_total.max(1);
    let target = (total as f64 * 0.95) as u64;
    let mut cumulative: u64 = 0;
    for (bin, count) in stats.diff_histogram.iter().enumerate() {
        cumulative += count;
        if cumulative >= target {
            return bin as f64;
        }
    }
    stats.diff_histogram.len() as f64 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_whitelist() -> Whitelist {
        serde_json::from_str(
            r#"{
                "schema_version": "1.0",
                "whitelist_id": "whitelist-d3d12-3d_scene",
                "scene": "3d_scene",
                "tier": "windows-d3d12-whitelist",
                "platform_pair": {
                    "baseline": { "engine": "wgpu", "api": "vulkan", "role": "reference" },
                    "candidate": { "engine": "diligent", "api": "d3d12", "role": "replacement" }
                },
                "thresholds": {
                    "ssim": 0.98,
                    "psnr_db": 30.0,
                    "mean_abs_diff": null,
                    "max_abs_diff": null,
                    "diff_histogram_p95": null,
                    "notes": "calibrated during M2a"
                },
                "categories": [
                    {
                        "id": "rasterization_rules",
                        "label": "光栅化规则",
                        "description": "d",
                        "policy": "whitelistable",
                        "expected_differences": [],
                        "notes": "n"
                    }
                ],
                "notes": "test"
            }"#,
        )
        .expect("sample parses")
    }

    #[test]
    fn identical_image_passes_ssim_and_psnr() {
        let w = 16u32;
        let h = 16u32;
        let data = vec![7u8; (w * h * 3) as usize];
        let stats = crate::metrics::compare_rgb(&data, &data, w, h).expect("valid");
        let verdict = judge(&stats, &sample_whitelist(), "0210");
        assert!(verdict.passed, "checks: {:?}", verdict.checks);
        assert_eq!(verdict.checks.len(), 2);
    }

    #[test]
    fn low_ssim_fails() {
        let w = 32u32;
        let h = 32u32;
        let a = vec![0u8; (w * h * 3) as usize];
        let b = vec![255u8; (w * h * 3) as usize];
        let stats = crate::metrics::compare_rgb(&a, &b, w, h).expect("valid");
        let verdict = judge(&stats, &sample_whitelist(), "0210");
        assert!(!verdict.passed);
        let ssim = verdict.checks.iter().find(|c| c.metric == "ssim").expect("ssim check");
        assert!(!ssim.passed);
    }

    #[test]
    fn per_frame_override_is_applied() {
        let mut whitelist = sample_whitelist();
        whitelist.thresholds.ssim = Some(0.99);
        whitelist.categories[0].expected_differences.push(ExpectedDifference {
            frame: "0210".to_string(),
            metric: "ssim".to_string(),
            observed: MetricValue::Number(0.985),
            threshold: 0.98,
            scope: None,
            rationale: "rasterization edge rule difference on this frame".to_string(),
        });
        // ssim = 0.985 passes the per-frame override (0.98) but not the global (0.99).
        let w = 16u32;
        let h = 16u32;
        let data = vec![9u8; (w * h * 3) as usize];
        let stats = crate::metrics::compare_rgb(&data, &data, w, h).expect("valid");
        let verdict = judge(&stats, &whitelist, "0210");
        let ssim = verdict.checks.iter().find(|c| c.metric == "ssim").expect("ssim check");
        assert_eq!(ssim.threshold, 0.98, "per-frame override must win");
    }

    #[test]
    fn histogram_p95_is_zero_for_identical() {
        let w = 16u32;
        let h = 16u32;
        let data = vec![3u8; (w * h * 3) as usize];
        let stats = crate::metrics::compare_rgb(&data, &data, w, h).expect("valid");
        assert_eq!(histogram_p95(&stats), 0.0);
    }

    #[test]
    fn metric_value_inf_parses() {
        let v: MetricValue = serde_json::from_str("\"inf\"").expect("parses");
        assert!(v.as_f64().is_infinite());
    }
}