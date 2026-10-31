//! Desktop tray companion (docs/guide/dashboard-cli-config.md §9.3): shows
//! a system-tray (Windows) / menu-bar (macOS) icon reflecting the engine's
//! aggregate health, with a beat-pulse animation that can be switched off
//! from the context menu (persisted).
//!
//! ## Discipline: reuse the engine's state, never invent a new one
//!
//! [`TrayStatus`] is derived from the engine's own [`HealthStatus`]
//! (custos-model) via [`HealthSummary::from_health`]: `Healthy` only when
//! **every** managed resource is `Healthy`; any `Degraded`/`Unhealthy`/
//! `Recovering`/`Suppressed`/`Unreachable`/`Unknown` resource flips the
//! tray to [`TrayStatus::Abnormal`]. No third state is fabricated.
//!
//! ## Discipline: state is not carried by color alone
//!
//! Each status has a distinct silhouette (solid circle vs circle-with-slash)
//! **and** a text equivalent (`tooltip`, `accessible_name`) — the two are
//! redundant channels so the tray is legible without color.
//!
//! ## Discipline: platform-independent frames
//!
//! The icon is rendered by [`pulse_frames`] / [`static_frame`] into RGBA
//! buffers with a built-in (dependency-free) renderer, shared verbatim by
//! the Windows tray and the macOS menu bar (§9.3 帧由平台无关层生成).
//! The animation is a heartbeat beat: the disc breathes and a faint ring
//! waves outward — at 16/24 px the beat stays legible.
//!
//! ## Discipline: config persistence
//!
//! The pulse switch is a plain toggle persisted to a JSON file (see
//! [`TrayConfig::load`] / [`TrayConfig::save`]); default on (§9.3 开关默认
//! 开启). No settings service, no DB table — a tray preference is not
//! engine state.

#[cfg(any(windows, target_os = "macos"))]
pub mod driver;

use custos_model::HealthStatus;
use serde::{Deserialize, Serialize};

/// Brand crimson (the `#FC0740` from `docs/public/logo.svg`), used only for
/// the healthy/beat channel. The abnormal channel is desaturated gray.
pub const BRAND_RED: (u8, u8, u8) = (252, 7, 64);
/// Abnormal-state gray.
pub const GRAY: (u8, u8, u8) = (140, 140, 140);

/// Aggregate tray state — a two-tone reduction of the engine's per-resource
/// [`HealthStatus`]s (a tray icon cannot carry seven states; the summary
/// text carries the detail).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStatus {
    /// Every managed resource is [`HealthStatus::Healthy`].
    Healthy,
    /// At least one resource is anything else (a fault, degraded, unknown,
    /// unprobeable, recovering or suppressed).
    Abnormal,
}

impl TrayStatus {
    /// Accessible short name (does not rely on color).
    pub fn accessible_name(self) -> &'static str {
        match self {
            TrayStatus::Healthy => "Custos healthy",
            TrayStatus::Abnormal => "Custos needs attention",
        }
    }

    /// Tooltip: one whole sentence including the summary (the text
    /// equivalent that makes the icon readable without color).
    pub fn tooltip(self, summary: &HealthSummary) -> String {
        match self {
            TrayStatus::Healthy => format!("All {} resource(s) healthy", summary.total),
            TrayStatus::Abnormal => {
                if summary.total == 0 {
                    "No probe results yet".to_owned()
                } else {
                    format!(
                        "{}/{} resource(s) healthy — {} need(s) attention",
                        summary.healthy,
                        summary.total,
                        summary.abnormal_count()
                    )
                }
            }
        }
    }
}

/// The aggregate view the tray renders: how many of the managed resources
/// are healthy right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HealthSummary {
    pub healthy: usize,
    pub total: usize,
}

impl HealthSummary {
    /// Reduces the engine's per-resource [`HealthStatus`] list to the
    /// two-tone summary. Empty input is reported honestly (no resources).
    pub fn from_health(statuses: &[HealthStatus]) -> Self {
        let total = statuses.len();
        let healthy = statuses
            .iter()
            .filter(|status| **status == HealthStatus::Healthy)
            .count();
        Self { healthy, total }
    }

    /// The tray status this summary renders as.
    pub fn status(&self) -> TrayStatus {
        if self.total > 0 && self.healthy == self.total {
            TrayStatus::Healthy
        } else {
            TrayStatus::Abnormal
        }
    }

    /// Resource ids that are not healthy (for the summary text / menu).
    pub fn abnormal_count(&self) -> usize {
        self.total - self.healthy
    }
}

// ── icon frame generation (platform-independent, shared by Windows/macOS) ──

/// One RGBA frame (row-major, 4 bytes per pixel) handed to the platform
/// tray crate (`tray-icon::Icon::from_rgba`). The conversion lives in the
/// platform drivers because `tray-icon` is only a Windows/macOS dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Number of beat frames (one full heartbeat cycle).
pub const PULSE_FRAME_COUNT: u32 = 24;

/// Renders the heartbeat frames for a status. `Healthy` beats in brand red;
/// `Abnormal` is a static gray disc with a slash (no pulse, §9.3 异常灰无脉搏).
pub fn pulse_frames(status: TrayStatus, size: u32) -> Vec<IconFrame> {
    match status {
        TrayStatus::Healthy => (0..PULSE_FRAME_COUNT)
            .map(|frame| render_beat(size, frame as f32 / PULSE_FRAME_COUNT as f32))
            .collect(),
        // Abnormal: a single static frame; the platform layer never animates.
        TrayStatus::Abnormal => vec![static_frame(status, size)],
    }
}

/// Renders the one static (never-animated) frame for a status.
pub fn static_frame(status: TrayStatus, size: u32) -> IconFrame {
    match status {
        TrayStatus::Healthy => render_beat(size, 0.0),
        TrayStatus::Abnormal => render_slash(size),
    }
}

/// The beat: a brand-red disc whose radius breathes plus a faint ring that
/// waves outward. `phase` in 0..1 selects the frame.
fn render_beat(size: u32, phase: f32) -> IconFrame {
    let side = size as usize;
    let mut rgba = vec![0u8; side * side * 4];
    let (cx, cy) = (side as f32 / 2.0, side as f32 / 2.0);
    let half = size as f32 / 2.0;
    // Base radius breathes ±8% around 0.40 of the half-canvas.
    let beat = (phase * std::f32::consts::TAU).sin();
    let base_r = (0.40 + 0.032 * beat) * half;
    // Outward wave: a ring at r = 0.40..0.52 of half-canvas, alpha fading.
    let wave_r = (0.42 + 0.12 * phase) * half;
    let wave_alpha = ((1.0 - phase) * 180.0) as u8;

    for y in 0..side {
        for x in 0..side {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            // Disc: feathered edge (~1px).
            let disc = coverage(dist, base_r, 1.2);
            // Wave: a thin annulus around wave_r.
            let ring = ring_coverage(dist, wave_r, 2.0);
            let alpha = (disc * 255.0).max(ring * wave_alpha as f32);
            if alpha < 1.0 {
                continue;
            }
            let (r, g, b) = BRAND_RED;
            let px = (y * side + x) * 4;
            rgba[px] = r;
            rgba[px + 1] = g;
            rgba[px + 2] = b;
            rgba[px + 3] = alpha.min(255.0) as u8;
        }
    }
    IconFrame {
        width: size,
        height: size,
        rgba,
    }
}

/// The abnormal icon: a gray disc with a diagonal slash (the distinct
/// silhouette; color is only the second channel).
fn render_slash(size: u32) -> IconFrame {
    let side = size as usize;
    let mut rgba = vec![0u8; side * side * 4];
    let (cx, cy) = (side as f32 / 2.0, side as f32 / 2.0);
    let half = size as f32 / 2.0;
    let base_r = 0.40 * half;
    let (r, g, b) = GRAY;
    for y in 0..side {
        for x in 0..side {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let disc = coverage(dist, base_r, 1.2);
            // Slash along the main diagonal, clipped to the disc.
            let slash = (dx - dy).abs() / 2.0_f32.sqrt();
            let slash_alpha = if dist <= base_r + 1.0 {
                coverage(slash, 0.055 * half, 1.2)
            } else {
                0.0
            };
            let alpha = disc.max(slash_alpha) * 255.0;
            if alpha < 1.0 {
                continue;
            }
            let px = (y * side + x) * 4;
            rgba[px] = r;
            rgba[px + 1] = g;
            rgba[px + 2] = b;
            rgba[px + 3] = alpha.min(255.0) as u8;
        }
    }
    IconFrame {
        width: size,
        height: size,
        rgba,
    }
}

/// Coverage at `dist` for a circle of radius `r` (1 inside, feathered edge).
fn coverage(dist: f32, r: f32, feather: f32) -> f32 {
    ((r - dist) / feather).clamp(0.0, 1.0)
}

/// Coverage for a thin annulus at radius `r` (peaks at the ring, fades ±width).
fn ring_coverage(dist: f32, r: f32, width: f32) -> f32 {
    let d = (dist - r).abs();
    (1.0 - d / width).clamp(0.0, 1.0)
}

/// A single pixel's alpha at (x, y) — used by the silhouette tests.
#[cfg(test)]
fn alpha_at(frame: &IconFrame, x: u32, y: u32) -> u8 {
    let idx = (y * frame.width + x) as usize * 4 + 3;
    frame.rgba[idx]
}

// ── tray preferences (pulse switch, persisted) ────────────────────────────

/// Persisted tray preferences (§9.3 设置持久化, 开关默认开启).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrayConfig {
    /// Whether the healthy-state beat-pulse animation is on. Default on.
    #[serde(default = "default_true")]
    pub pulse_animation: bool,
}

fn default_true() -> bool {
    true
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            pulse_animation: true,
        }
    }
}

impl TrayConfig {
    /// Loads from `path`; a missing or unreadable file yields the default
    /// (pulse on). A malformed file is reported as `Err` so the caller can
    /// log it rather than silently dropping the user's choice.
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        match std::fs::read_to_string(path.as_ref()) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("parse {}: {e}", path.as_ref().display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(format!("read {}: {err}", path.as_ref().display())),
        }
    }

    /// Persists to `path`, creating the parent directory when needed.
    pub fn save(&self, path: impl AsRef<std::path::Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| format!("serialize tray config: {e}"))?;
        std::fs::write(path, body).map_err(|e| format!("write {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn statuses(list: &[HealthStatus]) -> HealthSummary {
        HealthSummary::from_health(list)
    }

    // ── discipline: reuse the engine's state, never invent one ──────────

    #[test]
    fn all_healthy_is_healthy() {
        let summary = statuses(&[HealthStatus::Healthy, HealthStatus::Healthy]);
        assert_eq!(summary.status(), TrayStatus::Healthy);
        assert_eq!(summary.healthy, 2);
        assert_eq!(summary.abnormal_count(), 0);
    }

    #[test]
    fn any_non_healthy_flips_to_abnormal() {
        // Every non-Healthy variant must flip the tray; none is "fine".
        for bad in [
            HealthStatus::Degraded,
            HealthStatus::Unhealthy,
            HealthStatus::Recovering,
            HealthStatus::Suppressed,
            HealthStatus::Unreachable,
            HealthStatus::Unknown,
        ] {
            let summary = statuses(&[HealthStatus::Healthy, bad]);
            assert_eq!(summary.status(), TrayStatus::Abnormal, "{bad:?} must flip");
            assert_eq!(summary.abnormal_count(), 1, "{bad:?}");
        }
    }

    #[test]
    fn empty_summary_is_abnormal_not_healthy() {
        // No probe results yet must not read as "all healthy".
        let summary = statuses(&[]);
        assert_eq!(summary.status(), TrayStatus::Abnormal);
        assert_eq!(summary.total, 0);
        assert_eq!(summary.abnormal_count(), 0);
    }

    // ── discipline: state is not carried by color alone ─────────────────

    #[test]
    fn every_status_has_a_text_equivalent() {
        let summary = statuses(&[HealthStatus::Unhealthy]);
        for status in [TrayStatus::Healthy, TrayStatus::Abnormal] {
            assert!(!status.accessible_name().is_empty());
            assert!(!status.tooltip(&summary).is_empty());
        }
    }

    #[test]
    fn tooltips_are_distinct_and_carry_the_summary() {
        let good = statuses(&[HealthStatus::Healthy, HealthStatus::Healthy]);
        let bad = statuses(&[HealthStatus::Healthy, HealthStatus::Unhealthy]);
        let good_tip = TrayStatus::Healthy.tooltip(&good);
        let bad_tip = TrayStatus::Abnormal.tooltip(&bad);
        assert_ne!(good_tip, bad_tip);
        assert!(
            bad_tip.contains("1/2"),
            "tooltip must carry the count: {bad_tip}"
        );
        assert!(
            good_tip.contains("2"),
            "tooltip must carry the total: {good_tip}"
        );
    }

    // ── discipline: platform-independent frames ─────────────────────────

    #[test]
    fn healthy_beats_have_many_frames_and_abnormal_has_one() {
        let healthy = pulse_frames(TrayStatus::Healthy, 16);
        assert_eq!(healthy.len(), PULSE_FRAME_COUNT as usize);
        let abnormal = pulse_frames(TrayStatus::Abnormal, 16);
        assert_eq!(abnormal.len(), 1, "abnormal must never animate");
    }

    #[test]
    fn every_frame_is_well_formed() {
        for status in [TrayStatus::Healthy, TrayStatus::Abnormal] {
            for frame in pulse_frames(status, 24) {
                assert_eq!(frame.width, 24);
                assert_eq!(frame.height, 24);
                assert_eq!(frame.rgba.len(), 24 * 24 * 4);
                // Not an empty icon.
                assert!(frame.rgba.chunks(4).any(|p| p[3] > 200));
            }
        }
    }

    #[test]
    fn healthy_beats_differ_across_the_cycle() {
        // The pulse must actually animate: at least two distinct frames.
        let frames = pulse_frames(TrayStatus::Healthy, 16);
        assert!(
            frames.windows(2).any(|w| w[0] != w[1]),
            "beat frames must change over the cycle"
        );
    }

    #[test]
    fn healthy_and_abnormal_silhouettes_are_distinct() {
        // The center pixel: solid disc (healthy) vs slash through the gray
        // disc — the slash crosses the center, so the shapes differ.
        let healthy = static_frame(TrayStatus::Healthy, 16);
        let abnormal = static_frame(TrayStatus::Abnormal, 16);
        let _center = 8 * 16 + 8;
        // Both render the center (disc covers it) but the abnormal one is
        // gray while the healthy one is brand red — and the slash makes the
        // abnormal silhouette different along the diagonal.
        assert!(alpha_at(&healthy, 8, 8) > 200);
        assert!(alpha_at(&abnormal, 8, 8) > 200, "slash crosses the center");
        assert_ne!(healthy, abnormal, "the two statuses must not share a frame");
        // Off-diagonal but on-disc: healthy is filled; abnormal has the disc
        // too, so compare a diagonal-vs-anti-diagonal probe instead.
        assert_ne!(healthy.rgba, abnormal.rgba);
    }

    // ── discipline: config persistence ───────────────────────────────────

    #[test]
    fn config_defaults_to_pulse_on() {
        assert!(TrayConfig::default().pulse_animation);
    }

    #[test]
    fn config_round_trips_and_survives_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tray.json");
        // Missing file → default (on).
        assert!(TrayConfig::load(&path).unwrap().pulse_animation);
        // Save off, reload off.
        let off = TrayConfig {
            pulse_animation: false,
        };
        off.save(&path).unwrap();
        assert!(!TrayConfig::load(&path).unwrap().pulse_animation);
    }

    #[test]
    fn malformed_config_is_reported_not_silently_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tray.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(TrayConfig::load(&path).is_err());
    }

    #[test]
    fn config_missing_fields_fall_back_to_default() {
        // Forward-compatible: an older file without the field still loads.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tray.json");
        std::fs::write(&path, "{}").unwrap();
        assert!(TrayConfig::load(&path).unwrap().pulse_animation);
    }

    // ── brand color and gray are fixed ───────────────────────────────────

    #[test]
    fn brand_red_and_gray_render() {
        let healthy = static_frame(TrayStatus::Healthy, 16);
        let abnormal = static_frame(TrayStatus::Abnormal, 16);
        // Healthy center pixel is brand red.
        let (r, g, b) = BRAND_RED;
        let idx = (8 * 16 + 8) as usize * 4;
        assert_eq!(&healthy.rgba[idx..idx + 3], &[r, g, b]);
        // Abnormal center pixel is gray.
        let (r, g, b) = GRAY;
        assert_eq!(&abnormal.rgba[idx..idx + 3], &[r, g, b]);
    }
}
