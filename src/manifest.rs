//! SPEC_Build.md §3.1: `manifest.json`, written next to the world folder.
//! Records what a `--input-source kr` run was generated from and, crucially,
//! the coordinate origin -- without it a block position can never be traced
//! back to a real-world coordinate (SPEC_Build.md §3.1: "이 값이 있으면 언제든
//! 블록 좌표와 실제 좌표를 오갈 수 있다").
//!
//! Only written for `--input-source kr` (SPEC_Build.md M0's scope): the
//! `origin` block is EPSG:5186-specific and has no equivalent for the
//! existing OSM/local path.

use serde::Serialize;
use serde_json::json;
use std::io;
use std::path::Path;

#[derive(Serialize)]
pub struct ManifestOrigin {
    pub epsg: u32,
    /// Real-world easting (m) of the reference point -- SPEC_Ingest.md §2.2's `E0`.
    pub e0: f64,
    /// Real-world northing (m) of the reference point -- SPEC_Ingest.md §2.2's `N0`.
    pub n0: f64,
    /// Minimum source elevation (m) in the target area -- SPEC_Ingest.md §2.2's `H0`.
    /// `None` when elevation was disabled (flat ground has no real minimum).
    pub h0: Option<f64>,
    pub y2_base: i32,
}

/// SPEC_Ingest.md §2.3 "고도 압축 매핑": the parameters needed to invert a
/// block Y back to a real-world elevation once compression fired. Paired
/// with `origin.h0` (the bottom of the mapping) the same way `origin` itself
/// is paired with `bbox` -- neither reads on its own without the other.
/// `None` when elevation was disabled (nothing was mapped, let alone
/// compressed).
#[derive(Serialize)]
pub struct ManifestElevationMapping {
    /// `H_LINEAR` (m): elevation up to which `scale` applied in full.
    pub h_linear: f64,
    /// Solved compression coefficient `k`. `0.0` means the relief fit the
    /// available Y-range at full `scale` and no compression ran.
    pub compression: f64,
    /// The raw grid's maximum elevation (m) -- what `compression` was solved
    /// against.
    pub max_source_elevation: f64,
}

pub struct Manifest {
    pub scale: f64,
    pub origin: ManifestOrigin,
    /// SPEC_Ingest.md §2.3. `None` when elevation was disabled.
    pub elevation_mapping: Option<ManifestElevationMapping>,
    /// Block-coordinate extents `[x_min, z_min, x_max, z_max]` (SPEC_Build.md §3.1).
    pub bbox: [i32; 4],
    /// Data sources actually used this run, not a fixed template -- M0 only
    /// touches elevation, so listing 표준노드링크/건물통합정보/버스노선 here
    /// (all still M1/M4 work) would claim a source that was never fetched.
    pub sources: Vec<serde_json::Value>,
}

/// The actual on-disk shape (SPEC_Build.md §3.1) -- a private mirror of
/// `Manifest` so `generated_at`/`parameters` don't need to be filled in (and
/// kept in sync by hand) every place a `Manifest` gets constructed.
#[derive(Serialize)]
struct ManifestDocument<'a> {
    generated_at: String,
    scale: f64,
    origin: &'a ManifestOrigin,
    elevation_mapping: &'a Option<ManifestElevationMapping>,
    bbox: [i32; 4],
    sources: &'a [serde_json::Value],
    parameters: serde_json::Value,
}

impl Manifest {
    /// Writes `manifest.json` into `dir` (SPEC_Build.md §3: "월드 폴더와 나란히
    /// 둔다" -- the caller passes the world's parent directory, not the world
    /// folder itself).
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        let doc = ManifestDocument {
            generated_at: now_iso8601_utc(),
            scale: self.scale,
            origin: &self.origin,
            elevation_mapping: &self.elevation_mapping,
            bbox: self.bbox,
            sources: &self.sources,
            parameters: serde_json::json!({}),
        };
        let text = serde_json::to_string_pretty(&doc)
            .expect("manifest fields are all JSON-safe by construction");
        std::fs::write(dir.join("manifest.json"), text)
    }
}

/// A `sources[]` entry for the elevation data actually used
/// (SPEC_Build.md §3.1's `{"name": "고도데이터", "source": "arnis-builtin", ...}`).
/// `version` is this fork's own crate version, not the elevation provider's --
/// which provider actually served the request (Mapterhorn, USGS 3DEP, ...)
/// is decided deep inside `ground::generate_ground_data` and only printed to
/// stdout (`elevation::selector::select_provider`), not returned to the
/// caller, so it isn't threaded through to here for M0.
/// A `sources[]` entry for 표준노드링크 (SPEC_Build.md §3.1's own schema
/// example: `{"name": "표준노드링크", "version": "...", "retrieved": "..."}`).
/// `version` has no real dataset version to report (the distribution isn't
/// itself versioned the way a software release is) -- `dir_label`, when
/// given, is the source directory's own name (e.g. this project's
/// `[2026-08-12]NODELINKDATA` folder already encodes an acquisition date),
/// which is a more useful provenance string than this crate's own version.
pub fn road_network_source_entry(dir_label: Option<&str>) -> serde_json::Value {
    json!({
        "name": "표준노드링크",
        "version": dir_label.unwrap_or(env!("CARGO_PKG_VERSION")),
        "retrieved": now_iso8601_utc(),
    })
}

pub fn elevation_source_entry() -> serde_json::Value {
    json!({
        "name": "고도데이터",
        "source": "arnis-builtin",
        "version": env!("CARGO_PKG_VERSION"),
        "retrieved": now_iso8601_utc(),
    })
}

/// A `sources[]` entry for the bus route data actually used
/// (SPEC_Build.md §3.1: "`sources`의 기준일도 필수다. 버스 노선은 2025년
/// 개편 이후 여러 차례 바뀌었으므로, 어느 시점 기준인지가 기록되지 않으면
/// 재현이 불가능하다"). `baseline_date` is the source data's own as-of date
/// (e.g. `kr_bus_routes::REFERENCE_DATE`), not today's date -- `retrieved`
/// already covers when this run happened.
pub fn bus_route_source_entry(baseline_date: &str) -> serde_json::Value {
    json!({
        "name": "버스노선",
        "baseline_date": baseline_date,
        "retrieved": now_iso8601_utc(),
    })
}

fn now_iso8601_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    unix_seconds_to_iso8601_utc(secs)
}

fn unix_seconds_to_iso8601_utc(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let secs_of_day = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    let ss = secs_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Howard Hinnant's `civil_from_days`: converts a days-since-epoch count into
/// a (proleptic Gregorian) year/month/day. No external date/time crate is a
/// dependency of this project, and a manifest timestamp doesn't need one --
/// see <https://howardhinnant.github.io/date_algorithms.html>.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_1970_01_01() {
        assert_eq!(unix_seconds_to_iso8601_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_date_roundtrips() {
        // Every pair here was produced independently by GNU coreutils' `date`
        // (`date -u -d @<secs> +"%Y-%m-%dT%H:%M:%SZ"`), not derived from this
        // file's own formula. Includes a Feb-29-of-a-century-that-is-a-leap-
        // year case (2000, divisible by 400) since that's exactly where a
        // naive "divisible by 4" leap rule would drift.
        let cases: &[(i64, &str)] = &[
            (1_789_101_141, "2026-09-11T04:32:21Z"),
            (1_000_000_000, "2001-09-09T01:46:40Z"),
            (1_709_251_200, "2024-03-01T00:00:00Z"),
            (1_735_689_600, "2025-01-01T00:00:00Z"),
            (951_782_400, "2000-02-29T00:00:00Z"),
        ];
        for &(secs, expected) in cases {
            assert_eq!(unix_seconds_to_iso8601_utc(secs), expected);
        }
    }

    #[test]
    fn month_and_day_are_never_zero_or_out_of_range() {
        // Sweep a few years' worth of days and check the invariants hold,
        // rather than trust the algorithm on a single fixed point.
        let mut secs = 0_i64;
        for _ in 0..(365 * 5) {
            let (_, m, d) = civil_from_days(secs.div_euclid(86400));
            assert!((1..=12).contains(&m), "month out of range: {m}");
            assert!((1..=31).contains(&d), "day out of range: {d}");
            secs += 86400;
        }
    }
}
