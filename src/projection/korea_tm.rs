use super::Projection;
use crate::coordinate_system::geographic::LLBBox;

/// GRS80 ellipsoid parameters (KGD2002 datum, EPSG:4737 base CRS of EPSG:5186),
/// read from the EPSG registry's own PROJJSON (`https://epsg.io/5186.json`) --
/// not assumed.
const GRS80_A: f64 = 6_378_137.0;
const GRS80_INV_F: f64 = 298.257_222_101;

/// EPSG:5186 "KGD2002 / Central Belt 2010" Transverse Mercator parameters,
/// confirmed against the same PROJJSON (method EPSG:9807, parameters
/// EPSG:8801/8802/8805/8806/8807).
const LAT0_DEG: f64 = 38.0;
const LON0_DEG: f64 = 127.0;
const K0: f64 = 1.0;
const FALSE_EASTING: f64 = 200_000.0;
const FALSE_NORTHING: f64 = 600_000.0;

struct Ellipsoid {
    a: f64,
    e2: f64,
}

fn grs80() -> Ellipsoid {
    let f = 1.0 / GRS80_INV_F;
    Ellipsoid {
        a: GRS80_A,
        e2: f * (2.0 - f),
    }
}

/// SPEC_Ingest.md §2: Korea Central Belt (EPSG:5186) Transverse Mercator on the
/// GRS80 ellipsoid, composed with the §2.2 block-coordinate formula:
///
/// ```text
/// block_x = round((E - E0) * SCALE)
/// block_z = round((N0 - N) * SCALE)      // north = -Z
/// ```
///
/// `E0`/`N0` are the projected easting/northing of a reference point (SPEC:
/// "대상 영역 서남단 기준점", the target area's southwest corner) so that point
/// maps to Minecraft `(0, 0)` -- the same convention `--projection local`
/// already uses for its bbox corner.
///
/// KGD2002 and WGS84 are treated as coincident (SPEC_Ingest §2.1 note: read the
/// file's actual CRS and convert, don't assume -- but incoming `--bbox` values
/// are WGS84 by construction here, and the two datums agree to within
/// centimetres, far under a single block).
///
/// No `proj`/GDAL binding is used (SPEC_Ingest §8 left this undecided): the
/// projection is a pure, dependency-free implementation of the standard
/// Snyder ellipsoidal Transverse Mercator series (accurate to sub-millimetre
/// within a few degrees of the central meridian -- Korea's belt system is
/// narrower than that), which avoids adding a system library dependency to
/// the Windows build.
pub struct KoreaTmProjection {
    e0: f64,
    n0: f64,
    scale: f64,
}

impl KoreaTmProjection {
    /// `origin_lat`/`origin_lon` (WGS84 degrees) should be the target area's
    /// southwest corner -- SPEC_Ingest §2.2's `E0`/`N0`. `scale` is blocks per
    /// metre (SPEC_Ingest §2.2's `SCALE`, 1.75).
    pub fn new(origin_lat: f64, origin_lon: f64, scale: f64) -> Self {
        let (e0, n0) = tm_forward(origin_lat, origin_lon);
        Self { e0, n0, scale }
    }

    /// Same as [`Self::new`], but the reference point is already known in
    /// EPSG:5186 easting/northing metres -- the planar-native path
    /// (`--bbox-en`, or a [`KoreaPlanarBBox`] resolved once from lat/lon).
    /// Building the projection this way, instead of re-deriving `E0`/`N0`
    /// from lat/lon every time, is what keeps the origin exact: `E0`/`N0`
    /// are just copied, not recomputed through the forward series again.
    pub fn with_origin_en(e0: f64, n0: f64, scale: f64) -> Self {
        Self { e0, n0, scale }
    }

    /// Raw EPSG:5186 easting/northing in metres for an arbitrary WGS84 point,
    /// before the §2.2 origin shift and `SCALE`. Used by `main.rs` to fill
    /// `manifest.json`'s `origin.e0`/`origin.n0` with the actual metre values
    /// (SPEC_Build §3.1: "이 값이 있으면 언제든 블록 좌표와 실제 좌표를 오갈 수 있다").
    pub fn project_raw(lat: f64, lon: f64) -> (f64, f64) {
        tm_forward(lat, lon)
    }

    /// The reference point's projected easting/northing in metres -- what
    /// `manifest.json` records as `origin.e0`/`origin.n0`.
    pub fn origin_en(&self) -> (f64, f64) {
        (self.e0, self.n0)
    }
}

/// An axis-aligned rectangle in EPSG:5186 easting/northing metres -- the
/// canonical representation of "the area to generate" for `--input-source
/// kr`, instead of a lat/lon [`LLBBox`].
///
/// A lat/lon rectangle is the wrong type for this away from the central
/// meridian (127E): meridian convergence means a lat/lon rectangle's own
/// corners do not form a rectangle in E/N space -- projecting all four
/// independently (the general `CoordTransformer::with_projection` path)
/// yields a sheared quadrilateral whose axis-aligned envelope is measurably
/// wider than the intended square (confirmed against `proj4`: a 1500m
/// square 2 degrees east of the central meridian came out 2731x2626 blocks
/// at 1.75 blocks/metre instead of 2625x2625). `KoreaPlanarBBox` sidesteps
/// the question entirely: it describes the area directly in the metric
/// space the block grid is linear in, so `CoordTransformer::from_planar_extents`
/// never has to reconcile four independently-projected corners.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KoreaPlanarBBox {
    e_min: f64,
    n_min: f64,
    e_max: f64,
    n_max: f64,
}

impl KoreaPlanarBBox {
    pub fn new(e_min: f64, n_min: f64, e_max: f64, n_max: f64) -> Result<Self, String> {
        if !(e_min.is_finite() && n_min.is_finite() && e_max.is_finite() && n_max.is_finite()) {
            return Err("KoreaPlanarBBox: all four values must be finite".to_string());
        }
        if e_min >= e_max {
            return Err(format!(
                "KoreaPlanarBBox: e_min {e_min} >= e_max {e_max}"
            ));
        }
        if n_min >= n_max {
            return Err(format!(
                "KoreaPlanarBBox: n_min {n_min} >= n_max {n_max}"
            ));
        }
        Ok(Self {
            e_min,
            n_min,
            e_max,
            n_max,
        })
    }

    /// Parses `"e_min,n_min,e_max,n_max"` (metres, EPSG:5186) -- the
    /// `--bbox-en` CLI format, mirroring [`LLBBox::from_str`]'s comma/space
    /// tolerance so both flags feel the same to type.
    pub fn from_str(s: &str) -> Result<Self, String> {
        let mut values: Vec<f64> = Vec::with_capacity(4);
        for field in s.split([',', ' ']).filter(|f| !f.is_empty()) {
            let value: f64 = field
                .parse()
                .map_err(|_| format!("Invalid KoreaPlanarBBox: '{field}' is not a number"))?;
            values.push(value);
        }
        let [e_min, n_min, e_max, n_max]: [f64; 4] = values.try_into().map_err(|v: Vec<f64>| {
            format!("Invalid KoreaPlanarBBox: expected 4 values, got {}", v.len())
        })?;
        Self::new(e_min, n_min, e_max, n_max)
    }

    /// The one-time lat/lon -> planar conversion (SPEC_Ingest.md §2.1: read
    /// and convert once, don't assume/re-derive). Projects all four corners
    /// of `llbbox` via the raw TM forward series and takes their envelope --
    /// this is the same "widen to cover a sheared quadrilateral" the generic
    /// projection path does, but done exactly once here, at the entry point,
    /// rather than repeated at every one of the four call sites that used to
    /// each independently rebuild a transformer from the same `LLBBox`.
    pub fn from_llbbox(llbbox: &LLBBox) -> Self {
        let corners = [
            (llbbox.min().lat(), llbbox.min().lng()),
            (llbbox.min().lat(), llbbox.max().lng()),
            (llbbox.max().lat(), llbbox.min().lng()),
            (llbbox.max().lat(), llbbox.max().lng()),
        ];
        let mut e_min = f64::INFINITY;
        let mut e_max = f64::NEG_INFINITY;
        let mut n_min = f64::INFINITY;
        let mut n_max = f64::NEG_INFINITY;
        for (lat, lon) in corners {
            let (e, n) = tm_forward(lat, lon);
            e_min = e_min.min(e);
            e_max = e_max.max(e);
            n_min = n_min.min(n);
            n_max = n_max.max(n);
        }
        // Corners of a valid, non-degenerate LLBBox always produce a
        // non-degenerate envelope, so this cannot fail in practice; unwrap
        // rather than thread a Result through a conversion that is
        // conceptually infallible for real input.
        Self::new(e_min, n_min, e_max, n_max)
            .expect("LLBBox corners must project to a non-degenerate envelope")
    }

    pub fn e_min(&self) -> f64 {
        self.e_min
    }

    pub fn n_min(&self) -> f64 {
        self.n_min
    }

    pub fn e_max(&self) -> f64 {
        self.e_max
    }

    pub fn n_max(&self) -> f64 {
        self.n_max
    }

    pub fn width_m(&self) -> f64 {
        self.e_max - self.e_min
    }

    pub fn height_m(&self) -> f64 {
        self.n_max - self.n_min
    }
}

impl Projection for KoreaTmProjection {
    fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        let (e, n) = tm_forward(lat, lon);
        let x = (e - self.e0) * self.scale;
        let z = (self.n0 - n) * self.scale;
        (x, z)
    }

    fn inverse(&self, x: f64, z: f64) -> (f64, f64) {
        let e = x / self.scale + self.e0;
        let n = self.n0 - z / self.scale;
        tm_inverse(e, n)
    }
}

/// Snyder (1987) ellipsoidal Transverse Mercator forward series -- the same
/// formula PROJ's `tmerc` (non-`approx`) uses for zones narrow enough that the
/// series converges this fast. Returns (easting, northing) in metres.
fn tm_forward(lat_deg: f64, lon_deg: f64) -> (f64, f64) {
    let Ellipsoid { a, e2 } = grs80();
    let ep2 = e2 / (1.0 - e2);
    let phi = lat_deg.to_radians();
    let lambda = lon_deg.to_radians();
    let phi0 = LAT0_DEG.to_radians();
    let lambda0 = LON0_DEG.to_radians();

    let sin_phi = phi.sin();
    let cos_phi = phi.cos();
    let tan_phi = phi.tan();

    let n = a / (1.0 - e2 * sin_phi * sin_phi).sqrt();
    let t = tan_phi * tan_phi;
    let c = ep2 * cos_phi * cos_phi;
    let aa = (lambda - lambda0) * cos_phi;

    let m = meridian_arc(phi, a, e2);
    let m0 = meridian_arc(phi0, a, e2);

    let easting = FALSE_EASTING
        + K0 * n
            * (aa + (1.0 - t + c) * aa.powi(3) / 6.0
                + (5.0 - 18.0 * t + t * t + 72.0 * c - 58.0 * ep2) * aa.powi(5) / 120.0);

    let northing = FALSE_NORTHING
        + K0
            * (m - m0
                + n * tan_phi
                    * (aa * aa / 2.0
                        + (5.0 - t + 9.0 * c + 4.0 * c * c) * aa.powi(4) / 24.0
                        + (61.0 - 58.0 * t + t * t + 600.0 * c - 330.0 * ep2) * aa.powi(6)
                            / 720.0));

    (easting, northing)
}

/// Meridional arc length from the equator to latitude `phi` (radians).
fn meridian_arc(phi: f64, a: f64, e2: f64) -> f64 {
    let e4 = e2 * e2;
    let e6 = e4 * e2;
    a * ((1.0 - e2 / 4.0 - 3.0 * e4 / 64.0 - 5.0 * e6 / 256.0) * phi
        - (3.0 * e2 / 8.0 + 3.0 * e4 / 32.0 + 45.0 * e6 / 1024.0) * (2.0 * phi).sin()
        + (15.0 * e4 / 256.0 + 45.0 * e6 / 1024.0) * (4.0 * phi).sin()
        - (35.0 * e6 / 3072.0) * (6.0 * phi).sin())
}

/// Snyder (1987) ellipsoidal Transverse Mercator inverse series, matching
/// [`tm_forward`]. Returns (lat, lon) in WGS84 degrees.
fn tm_inverse(easting: f64, northing: f64) -> (f64, f64) {
    let Ellipsoid { a, e2 } = grs80();
    let ep2 = e2 / (1.0 - e2);
    let phi0 = LAT0_DEG.to_radians();
    let lambda0 = LON0_DEG.to_radians();

    let m0 = meridian_arc(phi0, a, e2);
    let m = m0 + (northing - FALSE_NORTHING) / K0;
    let e4 = e2 * e2;
    let e6 = e4 * e2;
    let mu = m / (a * (1.0 - e2 / 4.0 - 3.0 * e4 / 64.0 - 5.0 * e6 / 256.0));

    let e1 = (1.0 - (1.0 - e2).sqrt()) / (1.0 + (1.0 - e2).sqrt());

    let phi1 = mu
        + (3.0 * e1 / 2.0 - 27.0 * e1.powi(3) / 32.0) * (2.0 * mu).sin()
        + (21.0 * e1 * e1 / 16.0 - 55.0 * e1.powi(4) / 32.0) * (4.0 * mu).sin()
        + (151.0 * e1.powi(3) / 96.0) * (6.0 * mu).sin()
        + (1097.0 * e1.powi(4) / 512.0) * (8.0 * mu).sin();

    let sin_phi1 = phi1.sin();
    let cos_phi1 = phi1.cos();
    let tan_phi1 = phi1.tan();

    let c1 = ep2 * cos_phi1 * cos_phi1;
    let t1 = tan_phi1 * tan_phi1;
    let n1 = a / (1.0 - e2 * sin_phi1 * sin_phi1).sqrt();
    let r1 = a * (1.0 - e2) / (1.0 - e2 * sin_phi1 * sin_phi1).powf(1.5);
    let d = (easting - FALSE_EASTING) / (n1 * K0);

    let phi = phi1
        - (n1 * tan_phi1 / r1)
            * (d * d / 2.0
                - (5.0 + 3.0 * t1 + 10.0 * c1 - 4.0 * c1 * c1 - 9.0 * ep2) * d.powi(4) / 24.0
                + (61.0 + 90.0 * t1 + 298.0 * c1 + 45.0 * t1 * t1 - 252.0 * ep2 - 3.0 * c1 * c1)
                    * d.powi(6)
                    / 720.0);

    let lambda = lambda0
        + (d - (1.0 + 2.0 * t1 + c1) * d.powi(3) / 6.0
            + (5.0 - 2.0 * c1 + 28.0 * t1 - 3.0 * c1 * c1 + 8.0 * ep2 + 24.0 * t1 * t1) * d.powi(5)
                / 120.0)
            / cos_phi1;

    (phi.to_degrees(), lambda.to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference values from `proj4` (npm package `proj4` 2.x), using the exact
    // EPSG:5186 proj-string derived from the EPSG registry PROJJSON:
    // "+proj=tmerc +lat_0=38 +lon_0=127 +k=1 +x_0=200000 +y_0=600000
    //  +ellps=GRS80 +units=m +no_defs +type=crs"
    // These are an independent library's output, not derived from this file's
    // own formula -- computed once via `node -e` against `proj4` and pinned
    // here so a future edit can't silently drift.
    const REF_ORIGIN: (f64, f64, f64, f64) = (38.0, 127.0, 200_000.0000, 600_000.0000);
    const REF_YEONGSEON: (f64, f64, f64, f64) =
        (35.0835693, 129.0414405, 386_183.1762, 278_273.0711);
    const REF_BBOX_SW: (f64, f64, f64, f64) =
        (35.0735693, 129.0314405, 385_293.6467, 277_144.7702);
    const REF_BBOX_NE: (f64, f64, f64, f64) =
        (35.0935693, 129.0514405, 387_072.4790, 279_401.4699);

    // proj4 rounds to 1e-4 m in the reference dump; our series can differ from
    // that rounding by a similar amount, so 1 mm tolerance is the honest bar
    // (not the sub-mm the series is theoretically capable of).
    const TOL_M: f64 = 1.0e-3;

    fn assert_forward_matches(reference: (f64, f64, f64, f64)) {
        let (lat, lon, expected_e, expected_n) = reference;
        let (e, n) = tm_forward(lat, lon);
        assert!(
            (e - expected_e).abs() < TOL_M,
            "easting mismatch at ({lat},{lon}): got {e}, expected {expected_e}"
        );
        assert!(
            (n - expected_n).abs() < TOL_M,
            "northing mismatch at ({lat},{lon}): got {n}, expected {expected_n}"
        );
    }

    #[test]
    fn forward_matches_proj4_at_origin() {
        assert_forward_matches(REF_ORIGIN);
    }

    #[test]
    fn forward_matches_proj4_at_yeongseon_dong() {
        assert_forward_matches(REF_YEONGSEON);
    }

    #[test]
    fn forward_matches_proj4_at_bbox_corners() {
        assert_forward_matches(REF_BBOX_SW);
        assert_forward_matches(REF_BBOX_NE);
    }

    #[test]
    fn inverse_recovers_forward_input() {
        for &(lat, lon, _, _) in &[REF_ORIGIN, REF_YEONGSEON, REF_BBOX_SW, REF_BBOX_NE] {
            let (e, n) = tm_forward(lat, lon);
            let (lat2, lon2) = tm_inverse(e, n);
            assert!(
                (lat2 - lat).abs() < 1.0e-9,
                "lat roundtrip failed for ({lat},{lon}): got {lat2}"
            );
            assert!(
                (lon2 - lon).abs() < 1.0e-9,
                "lon roundtrip failed for ({lat},{lon}): got {lon2}"
            );
        }
    }

    #[test]
    fn block_formula_places_sw_corner_at_origin() {
        let (sw_lat, sw_lon, _, _) = REF_BBOX_SW;
        let proj = KoreaTmProjection::new(sw_lat, sw_lon, 1.75);
        let (x, z) = proj.forward(sw_lat, sw_lon);
        assert!(x.abs() < 1.0e-6, "expected x=0 at SW corner, got {x}");
        assert!(z.abs() < 1.0e-6, "expected z=0 at SW corner, got {z}");
    }

    #[test]
    fn block_formula_matches_spec_ingest_signs_and_scale() {
        // SPEC_Ingest §2.2: block_x = round((E-E0)*SCALE), block_z = round((N0-N)*SCALE).
        // East of the SW corner must give +x; north of it must give -z.
        let (sw_lat, sw_lon, _, _) = REF_BBOX_SW;
        let (ne_lat, ne_lon, _, _) = REF_BBOX_NE;
        let proj = KoreaTmProjection::new(sw_lat, sw_lon, 1.75);

        let (x_ne, z_ne) = proj.forward(ne_lat, ne_lon);
        assert!(x_ne > 0.0, "east of SW corner should be +x, got {x_ne}");
        assert!(z_ne < 0.0, "north of SW corner should be -z, got {z_ne}");

        // Cross-check against the raw proj4 E/N directly, scaled by hand.
        let (_, _, e_sw, n_sw) = REF_BBOX_SW;
        let (_, _, e_ne, n_ne) = REF_BBOX_NE;
        let expected_x = (e_ne - e_sw) * 1.75;
        let expected_z = (n_sw - n_ne) * 1.75;
        assert!(
            (x_ne - expected_x).abs() < 0.01,
            "x={x_ne}, expected~{expected_x}"
        );
        assert!(
            (z_ne - expected_z).abs() < 0.01,
            "z={z_ne}, expected~{expected_z}"
        );
    }

    #[test]
    fn origin_en_reports_raw_meters() {
        let (sw_lat, sw_lon, expected_e, expected_n) = REF_BBOX_SW;
        let proj = KoreaTmProjection::new(sw_lat, sw_lon, 1.75);
        let (e0, n0) = proj.origin_en();
        assert!((e0 - expected_e).abs() < TOL_M);
        assert!((n0 - expected_n).abs() < TOL_M);
    }

    #[test]
    fn with_origin_en_matches_new() {
        // Building from a lat/lon origin (`new`) or from that same origin's
        // already-known E/N (`with_origin_en`) must agree exactly -- the
        // whole point of the planar-native path is that this shortcut
        // changes nothing about what a point maps to.
        let (sw_lat, sw_lon, _, _) = REF_BBOX_SW;
        let by_latlon = KoreaTmProjection::new(sw_lat, sw_lon, 1.75);
        let (e0, n0) = by_latlon.origin_en();
        let by_en = KoreaTmProjection::with_origin_en(e0, n0, 1.75);

        let (ne_lat, ne_lon, _, _) = REF_BBOX_NE;
        assert_eq!(by_latlon.forward(ne_lat, ne_lon), by_en.forward(ne_lat, ne_lon));
    }

    // ----- KoreaPlanarBBox -----
    //
    // Reference envelope for the Yeongseon-dong 1500m square (same bbox as
    // transformation.rs's Korea TM tests), computed independently via
    // `proj4`: project all four lat/lon corners to EPSG:5186 and take min/max.
    //   sw=(385433.1762, 277523.0711) nw=(385403.2106, 278991.7203)
    //   ne=(386933.1762, 279023.0711) se=(386963.3894, 277554.4165)
    //   -> e in [385403.2106159030, 386963.3893671046]
    //      n in [277523.0710547101, 279023.0710547119]
    const YEONGSEON_SQUARE: (f64, f64, f64, f64) = (
        35.07695141877934,
        129.03305391424703,
        35.090186547431124,
        129.04982843930924,
    );
    const REF_ENVELOPE_E_MIN: f64 = 385_403.2106159030;
    const REF_ENVELOPE_E_MAX: f64 = 386_963.3893671046;
    const REF_ENVELOPE_N_MIN: f64 = 277_523.0710547101;
    const REF_ENVELOPE_N_MAX: f64 = 279_023.0710547119;

    #[test]
    fn planar_bbox_new_rejects_degenerate_or_non_finite() {
        assert!(KoreaPlanarBBox::new(0.0, 0.0, 0.0, 1.0).is_err());
        assert!(KoreaPlanarBBox::new(0.0, 0.0, 1.0, 0.0).is_err());
        assert!(KoreaPlanarBBox::new(1.0, 0.0, 0.0, 1.0).is_err());
        assert!(KoreaPlanarBBox::new(f64::NAN, 0.0, 1.0, 1.0).is_err());
        assert!(KoreaPlanarBBox::new(0.0, 0.0, 1.0, 1.0).is_ok());
    }

    #[test]
    fn planar_bbox_from_str_parses_comma_and_space() {
        let a = KoreaPlanarBBox::from_str("100,200,300,400").unwrap();
        let b = KoreaPlanarBBox::from_str("100 200 300 400").unwrap();
        let c = KoreaPlanarBBox::from_str("100, 200, 300, 400").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!((a.e_min(), a.n_min(), a.e_max(), a.n_max()), (100.0, 200.0, 300.0, 400.0));
    }

    #[test]
    fn planar_bbox_from_str_rejects_wrong_count() {
        assert!(KoreaPlanarBBox::from_str("1,2,3").is_err());
        assert!(KoreaPlanarBBox::from_str("1,2,3,4,5").is_err());
        assert!(KoreaPlanarBBox::from_str("a,b,c,d").is_err());
    }

    #[test]
    fn planar_bbox_from_llbbox_matches_proj4_envelope() {
        let (min_lat, min_lng, max_lat, max_lng) = YEONGSEON_SQUARE;
        let llbbox = LLBBox::new(min_lat, min_lng, max_lat, max_lng).unwrap();
        let planar = KoreaPlanarBBox::from_llbbox(&llbbox);

        assert!((planar.e_min() - REF_ENVELOPE_E_MIN).abs() < TOL_M);
        assert!((planar.e_max() - REF_ENVELOPE_E_MAX).abs() < TOL_M);
        assert!((planar.n_min() - REF_ENVELOPE_N_MIN).abs() < TOL_M);
        assert!((planar.n_max() - REF_ENVELOPE_N_MAX).abs() < TOL_M);
    }

    #[test]
    fn planar_bbox_width_height_in_meters() {
        // Confirms the shape the envelope actually has: wider than the
        // nominal 1500m (because of the NW/SE shear -- see the module doc),
        // but the height stays essentially exact because this bbox's height
        // bound happens to come from the untouched SW-NE diagonal.
        let (min_lat, min_lng, max_lat, max_lng) = YEONGSEON_SQUARE;
        let llbbox = LLBBox::new(min_lat, min_lng, max_lat, max_lng).unwrap();
        let planar = KoreaPlanarBBox::from_llbbox(&llbbox);

        assert!(
            (planar.width_m() - 1560.18).abs() < 0.1,
            "width_m={}, expected ~1560.18",
            planar.width_m()
        );
        assert!(
            (planar.height_m() - 1500.0).abs() < 0.1,
            "height_m={}, expected ~1500.0",
            planar.height_m()
        );
    }
}
