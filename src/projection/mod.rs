pub mod korea_tm;
pub mod web_mercator;

pub use korea_tm::{KoreaPlanarBBox, KoreaTmProjection};
pub use web_mercator::WebMercatorProjection;

use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::coordinate_system::transformation::CoordTransformer;
use std::fmt;
use std::str::FromStr;

/// Builds the `CoordTransformer` for a chosen `ProjectionKind`, `scale`, and
/// bbox. Every call site that needs a transformer for the CLI's `--projection`
/// choice goes through this, so a new `ProjectionKind` variant only has to be
/// handled in one place instead of once per call site (there were four:
/// `main.rs`'s spawn point, `osm_parser.rs`'s main dispatch, `landmarks.rs`,
/// `mapillary/mod.rs` -- all copies of the same match).
///
/// `korea_planar_bbox` is `Args::korea_planar_bbox` (SPEC_Ingest.md §2.1's
/// one-time lat/lon -> planar conversion, resolved once by
/// `apply_input_source_defaults` from `--bbox-en` or `--bbox`): every one of
/// the four call sites reads the *same* already-resolved rectangle here,
/// rather than each independently re-deriving an envelope from `bbox`. It is
/// only consulted for `ProjectionKind::KoreaTm`; the other kinds ignore it.
pub fn build_transformer(
    bbox: &LLBBox,
    kind: ProjectionKind,
    scale: f64,
    korea_planar_bbox: Option<KoreaPlanarBBox>,
) -> Result<(CoordTransformer, XZBBox), String> {
    match kind {
        ProjectionKind::WebMercator => {
            let origin_lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
            let origin_lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;
            let proj = WebMercatorProjection::new(origin_lat, origin_lon, scale);
            CoordTransformer::with_projection(bbox, scale, Box::new(proj))
        }
        ProjectionKind::Local => CoordTransformer::llbbox_to_xzbbox(bbox, scale),
        ProjectionKind::KoreaTm => {
            // SPEC_Ingest.md §2.2's E0/N0, taken directly from the already-
            // resolved planar bbox's own SW corner (not re-derived from
            // `bbox` here) -- see `CoordTransformer::from_planar_extents` for
            // why this, rather than `with_projection`, is what avoids the
            // meridian-convergence widening.
            let planar = korea_planar_bbox.ok_or_else(|| {
                "build_transformer: ProjectionKind::KoreaTm needs a resolved KoreaPlanarBBox \
                 (apply_input_source_defaults should have set Args::korea_planar_bbox)"
                    .to_string()
            })?;
            let proj =
                KoreaTmProjection::with_origin_en(planar.e_min(), planar.n_min(), scale);
            let width_blocks = (planar.width_m() * scale).round() as i32;
            let height_blocks = (planar.height_m() * scale).round() as i32;
            CoordTransformer::from_planar_extents(Box::new(proj), width_blocks, height_blocks)
        }
    }
}

/// Trait for converting between WGS84 geographic coordinates and a projected
/// coordinate system used in Minecraft world generation.
pub trait Projection {
    /// Convert WGS84 latitude/longitude (degrees) to projected (x, z) in meters
    /// (or blocks, depending on scale).
    fn forward(&self, lat: f64, lon: f64) -> (f64, f64);

    /// Convert projected (x, z) back to WGS84 latitude/longitude (degrees).
    /// Defined for completeness of the projection interface; current generation
    /// flow only needs `forward`, but reverse-projection is needed if we ever
    /// surface real-world coordinates back from a Minecraft point.
    #[allow(dead_code)]
    fn inverse(&self, x: f64, z: f64) -> (f64, f64);
}

/// Available map projection variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionKind {
    /// Web Mercator (EPSG:3857-like) projection with a local origin offset.
    WebMercator,
    /// Simple local coordinate system (no geographic projection).
    Local,
    /// SPEC_Ingest.md §2: Korea Central Belt (EPSG:5186) Transverse Mercator,
    /// selected automatically by `--input-source kr`.
    KoreaTm,
}

impl fmt::Display for ProjectionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectionKind::WebMercator => write!(f, "web_mercator"),
            ProjectionKind::Local => write!(f, "local"),
            ProjectionKind::KoreaTm => write!(f, "korea_tm"),
        }
    }
}

impl FromStr for ProjectionKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "web_mercator" | "webmercator" | "mercator" => Ok(ProjectionKind::WebMercator),
            "local" => Ok(ProjectionKind::Local),
            "korea_tm" | "koreatm" | "epsg:5186" | "epsg5186" => Ok(ProjectionKind::KoreaTm),
            other => Err(format!("unknown projection kind: '{other}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_projection_kind_display() {
        assert_eq!(ProjectionKind::WebMercator.to_string(), "web_mercator");
        assert_eq!(ProjectionKind::Local.to_string(), "local");
    }

    #[test]
    fn test_projection_kind_from_str() {
        assert_eq!(
            "web_mercator".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::WebMercator
        );
        assert_eq!(
            "webmercator".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::WebMercator
        );
        assert_eq!(
            "mercator".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::WebMercator
        );
        assert_eq!(
            "local".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::Local
        );
        assert_eq!(
            "LOCAL".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::Local
        );
    }

    #[test]
    fn test_projection_kind_from_str_invalid() {
        assert!("unknown".parse::<ProjectionKind>().is_err());
    }

    #[test]
    fn test_projection_kind_roundtrip() {
        for kind in [
            ProjectionKind::WebMercator,
            ProjectionKind::Local,
            ProjectionKind::KoreaTm,
        ] {
            let s = kind.to_string();
            let parsed: ProjectionKind = s.parse().unwrap();
            assert_eq!(parsed, kind);
        }
    }
}
