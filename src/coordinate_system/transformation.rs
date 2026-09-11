use super::cartesian::{XZBBox, XZPoint};
use super::geographic::{LLBBox, LLPoint};
use crate::projection::Projection;

/// Internal mode discriminator so `transform_point` can dispatch between the
/// legacy linear interpolation and an arbitrary geographic projection.
enum ProjectionMode {
    /// Existing linear-interpolation mode (no geographic projection).
    Local,
    /// Any `Projection` impl. Holds the actual projection object and calls
    /// `forward()` per point, rather than re-deriving its formula here --
    /// `with_projection` used to duplicate the Web Mercator math inline
    /// instead of calling back into the trait object, which meant a second
    /// projection type could be constructed via `with_projection` but would
    /// silently be transformed with Web Mercator's formula anyway. Storing
    /// the object itself removes that trap.
    Projected(Box<dyn Projection>),
}

/// Transform geographic space (within llbbox) to a local tangential cartesian space (within xzbbox)
pub struct CoordTransformer {
    len_lat: f64,
    len_lng: f64,
    scale_factor_x: f64,
    scale_factor_z: f64,
    min_lat: f64,
    min_lng: f64,
    mode: ProjectionMode,
}

impl CoordTransformer {
    pub fn scale_factor_x(&self) -> f64 {
        self.scale_factor_x
    }

    pub fn scale_factor_z(&self) -> f64 {
        self.scale_factor_z
    }

    pub fn llbbox_to_xzbbox(
        llbbox: &LLBBox,
        scale: f64,
    ) -> Result<(CoordTransformer, XZBBox), String> {
        let err_header = "Construct LLBBox to XZBBox transformation failed".to_string();

        if scale <= 0.0 {
            return Err(format!("{}: scale <= 0.0", err_header));
        }

        let (scale_factor_z, scale_factor_x) = geo_distance(llbbox.min(), llbbox.max());
        let scale_factor_z: f64 = scale_factor_z.floor() * scale;
        let scale_factor_x: f64 = scale_factor_x.floor() * scale;

        let xzbbox = XZBBox::rect_from_xz_lengths(scale_factor_x, scale_factor_z)
            .map_err(|e| format!("{}:\n{}", err_header, e))?;

        Ok((
            Self {
                len_lat: llbbox.max().lat() - llbbox.min().lat(),
                len_lng: llbbox.max().lng() - llbbox.min().lng(),
                scale_factor_x,
                scale_factor_z,
                min_lat: llbbox.min().lat(),
                min_lng: llbbox.min().lng(),
                mode: ProjectionMode::Local,
            },
            xzbbox,
        ))
    }

    /// Create a `CoordTransformer` from an arbitrary geographic `Projection`.
    ///
    /// The bounding box is computed by projecting all four corners of the
    /// `llbbox` and taking the axis-aligned envelope. The returned `XZBBox`
    /// represents the Minecraft world extents for the projected area. `scale`
    /// is only validated here -- the projection object is expected to have
    /// already baked its own scale into `forward()`/`inverse()`.
    pub fn with_projection(
        llbbox: &LLBBox,
        scale: f64,
        projection: Box<dyn Projection>,
    ) -> Result<(CoordTransformer, XZBBox), String> {
        if scale <= 0.0 {
            return Err("Scale must be > 0.0".to_string());
        }

        // Project all four corners to find the Minecraft bounding box.
        // NW corner
        let (x_nw, z_nw) = projection.forward(llbbox.max().lat(), llbbox.min().lng());
        // SE corner
        let (x_se, z_se) = projection.forward(llbbox.min().lat(), llbbox.max().lng());
        // NE corner
        let (x_ne, z_ne) = projection.forward(llbbox.max().lat(), llbbox.max().lng());
        // SW corner
        let (x_sw, z_sw) = projection.forward(llbbox.min().lat(), llbbox.min().lng());

        let x_min = x_nw.min(x_sw).min(x_ne).min(x_se).floor() as i32;
        let x_max = x_nw.max(x_sw).max(x_ne).max(x_se).ceil() as i32;
        let z_min = z_nw.min(z_sw).min(z_ne).min(z_se).floor() as i32;
        let z_max = z_nw.max(z_sw).max(z_ne).max(z_se).ceil() as i32;

        let xzbbox = XZBBox::rect_from_min_max(x_min, z_min, x_max, z_max)
            .map_err(|e| format!("Failed to create XZBBox from projection: {}", e))?;

        Ok((
            CoordTransformer {
                len_lat: llbbox.max().lat() - llbbox.min().lat(),
                len_lng: llbbox.max().lng() - llbbox.min().lng(),
                scale_factor_x: (x_max - x_min) as f64,
                scale_factor_z: (z_max - z_min) as f64,
                min_lat: llbbox.min().lat(),
                min_lng: llbbox.min().lng(),
                mode: ProjectionMode::Projected(projection),
            },
            xzbbox,
        ))
    }

    /// Create a `CoordTransformer` when the output block extents are already
    /// known exactly, instead of deriving them by projecting a geographic
    /// bbox's four corners. `width_blocks`/`height_blocks` must already be
    /// scaled (SPEC_Ingest.md's `SCALE`) -- this constructor does no scale
    /// arithmetic of its own, only shape.
    ///
    /// Use this for a projection whose "area to generate" is natively
    /// defined in the projection's own planar space (SPEC_Ingest.md's Korea
    /// TM path via `KoreaPlanarBBox`): projecting a lat/lon rectangle's four
    /// corners independently (`with_projection`, above) can yield a sheared
    /// quadrilateral whose envelope is wider than intended, away from a
    /// projection's central meridian (meridian convergence). Building the
    /// block `(0,0)` origin so it coincides with the projection's own
    /// `forward()` origin, and taking the extents directly, sidesteps that:
    /// there is only one rectangle in play, not four independently-projected
    /// corners to reconcile.
    pub fn from_planar_extents(
        projection: Box<dyn Projection>,
        width_blocks: i32,
        height_blocks: i32,
    ) -> Result<(CoordTransformer, XZBBox), String> {
        if width_blocks <= 0 || height_blocks <= 0 {
            return Err(format!(
                "Planar extents must be positive: width={width_blocks}, height={height_blocks}"
            ));
        }

        // The projection's own forward() origin (its E0/N0) is block (0,0);
        // north is -Z (SPEC_Ingest.md §2.2), so the rectangle extends from
        // (0, -height) to (width, 0).
        let xzbbox = XZBBox::rect_from_min_max(0, -height_blocks, width_blocks, 0)
            .map_err(|e| format!("Failed to create XZBBox from planar extents: {}", e))?;

        Ok((
            CoordTransformer {
                // Unused in `ProjectionMode::Projected` (see transform_point) --
                // there is no lat/lon rectangle here to compute a relative
                // position within.
                len_lat: 0.0,
                len_lng: 0.0,
                scale_factor_x: width_blocks as f64,
                scale_factor_z: height_blocks as f64,
                min_lat: 0.0,
                min_lng: 0.0,
                mode: ProjectionMode::Projected(projection),
            },
            xzbbox,
        ))
    }

    pub fn transform_point(&self, llpoint: LLPoint) -> XZPoint {
        match &self.mode {
            ProjectionMode::Local => {
                // Calculate the relative position within the bounding box
                let rel_x: f64 = (llpoint.lng() - self.min_lng) / self.len_lng;
                let rel_z: f64 = 1.0 - (llpoint.lat() - self.min_lat) / self.len_lat;

                // Apply scaling factors for each dimension and convert to Minecraft coordinates
                let x: i32 = (rel_x * self.scale_factor_x) as i32;
                let z: i32 = (rel_z * self.scale_factor_z) as i32;

                XZPoint::new(x, z)
            }
            ProjectionMode::Projected(projection) => {
                let (x, z) = projection.forward(llpoint.lat(), llpoint.lng());
                XZPoint::new(x as i32, z as i32)
            }
        }
    }
}

// (lat meters, lon meters)
#[inline]
pub fn geo_distance(a: LLPoint, b: LLPoint) -> (f64, f64) {
    let z: f64 = lat_distance(a.lat(), b.lat());

    // distance between two lons depends on their latitude. In this case we'll just average them
    let x: f64 = lon_distance((a.lat() + b.lat()) / 2.0, a.lng(), b.lng());

    (z, x)
}

// Haversine but optimized for a latitude delta of 0
// returns meters
fn lon_distance(lat: f64, lon1: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let d_lon: f64 = (lon2 - lon1).to_radians();
    let a: f64 =
        lat.to_radians().cos() * lat.to_radians().cos() * (d_lon / 2.0).sin() * (d_lon / 2.0).sin();
    let c: f64 = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

    R * c
}

// Haversine but optimized for a longitude delta of 0
// returns meters
fn lat_distance(lat1: f64, lat2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let d_lat: f64 = (lat2 - lat1).to_radians();
    let a: f64 = (d_lat / 2.0).sin() * (d_lat / 2.0).sin();
    let c: f64 = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

    R * c
}

// copied legacy code
// Function to convert latitude and longitude to Minecraft coordinates.
#[cfg(test)]
pub fn lat_lon_to_minecraft_coords(
    lat: f64,
    lon: f64,
    bbox: LLBBox, // (min_lon, min_lat, max_lon, max_lat)
    scale_factor_z: f64,
    scale_factor_x: f64,
) -> (i32, i32) {
    // Calculate the relative position within the bounding box
    let rel_x: f64 = (lon - bbox.min().lng()) / (bbox.max().lng() - bbox.min().lng());
    let rel_z: f64 = 1.0 - (lat - bbox.min().lat()) / (bbox.max().lat() - bbox.min().lat());

    // Apply scaling factors for each dimension and convert to Minecraft coordinates
    let x: i32 = (rel_x * scale_factor_x) as i32;
    let z: i32 = (rel_z * scale_factor_z) as i32;

    (x, z)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_utilities::get_llbbox_arnis;

    fn test_llxztransform_one_scale_one_factor(
        scale: f64,
        test_latfactor: f64,
        test_lngfactor: f64,
    ) {
        let llbbox = get_llbbox_arnis();
        let llpoint = LLPoint::new(
            llbbox.min().lat() + (llbbox.max().lat() - llbbox.min().lat()) * test_latfactor,
            llbbox.min().lng() + (llbbox.max().lng() - llbbox.min().lng()) * test_lngfactor,
        )
        .unwrap();
        let (transformer, xzbbox_new) = CoordTransformer::llbbox_to_xzbbox(&llbbox, scale).unwrap();

        // legacy xzbbox creation
        let (scale_factor_z, scale_factor_x) = geo_distance(llbbox.min(), llbbox.max());
        let scale_factor_z: f64 = scale_factor_z.floor() * scale;
        let scale_factor_x: f64 = scale_factor_x.floor() * scale;
        let xzbbox_old = XZBBox::rect_from_xz_lengths(scale_factor_x, scale_factor_z).unwrap();

        // legacy coord transform
        let (x, z) = lat_lon_to_minecraft_coords(
            llpoint.lat(),
            llpoint.lng(),
            llbbox,
            scale_factor_z,
            scale_factor_x,
        );
        // new coord transform
        let xzpoint = transformer.transform_point(llpoint);

        assert_eq!(x, xzpoint.x);
        assert_eq!(z, xzpoint.z);
        assert_eq!(xzbbox_new.min_x(), xzbbox_old.min_x());
        assert_eq!(xzbbox_new.max_x(), xzbbox_old.max_x());
        assert_eq!(xzbbox_new.min_z(), xzbbox_old.min_z());
        assert_eq!(xzbbox_new.max_z(), xzbbox_old.max_z());
    }

    // this ensures that transformer.transform_point == legacy lat_lon_to_minecraft_coords
    #[test]
    pub fn test_llxztransform() {
        test_llxztransform_one_scale_one_factor(1.0, 0.5, 0.5);
        test_llxztransform_one_scale_one_factor(3.0, 0.1, 0.2);
        test_llxztransform_one_scale_one_factor(10.0, -1.2, 2.0);
        test_llxztransform_one_scale_one_factor(0.4, 0.3, -0.2);
        test_llxztransform_one_scale_one_factor(0.1, 0.2, 0.7);
    }

    // this ensures that invalid inputs can be handled correctly
    #[test]
    pub fn test_invalid_construct() {
        let llbbox = get_llbbox_arnis();
        let obj = CoordTransformer::llbbox_to_xzbbox(&llbbox, 0.0);
        assert!(obj.is_err());

        let obj = CoordTransformer::llbbox_to_xzbbox(&llbbox, -1.2);
        assert!(obj.is_err());
    }

    // ----- Web Mercator projection mode tests -----

    #[test]
    fn test_with_projection_constructs_successfully() {
        let llbbox = get_llbbox_arnis();
        let proj = crate::projection::WebMercatorProjection::new(
            (llbbox.min().lat() + llbbox.max().lat()) / 2.0,
            (llbbox.min().lng() + llbbox.max().lng()) / 2.0,
            1.0,
        );
        let result = CoordTransformer::with_projection(&llbbox, 1.0, Box::new(proj));
        assert!(result.is_ok());
    }

    #[test]
    fn test_with_projection_invalid_scale() {
        let llbbox = get_llbbox_arnis();
        let proj_a = crate::projection::WebMercatorProjection::new(54.63, 9.93, 1.0);
        let proj_b = crate::projection::WebMercatorProjection::new(54.63, 9.93, 1.0);

        assert!(CoordTransformer::with_projection(&llbbox, 0.0, Box::new(proj_a)).is_err());
        assert!(CoordTransformer::with_projection(&llbbox, -1.0, Box::new(proj_b)).is_err());
    }

    #[test]
    fn test_with_projection_xzbbox_contains_projected_corners() {
        let llbbox = get_llbbox_arnis();
        let proj = crate::projection::WebMercatorProjection::new(
            (llbbox.min().lat() + llbbox.max().lat()) / 2.0,
            (llbbox.min().lng() + llbbox.max().lng()) / 2.0,
            1.0,
        );
        let (transformer, xzbbox) =
            CoordTransformer::with_projection(&llbbox, 1.0, Box::new(proj)).unwrap();

        // All four corners should map inside the xzbbox
        let corners = [
            LLPoint::new(llbbox.min().lat(), llbbox.min().lng()).unwrap(),
            LLPoint::new(llbbox.min().lat(), llbbox.max().lng()).unwrap(),
            LLPoint::new(llbbox.max().lat(), llbbox.min().lng()).unwrap(),
            LLPoint::new(llbbox.max().lat(), llbbox.max().lng()).unwrap(),
        ];

        for corner in &corners {
            let pt = transformer.transform_point(*corner);
            assert!(
                pt.x >= xzbbox.min_x() && pt.x <= xzbbox.max_x(),
                "x={} out of xzbbox [{}, {}] for corner ({}, {})",
                pt.x,
                xzbbox.min_x(),
                xzbbox.max_x(),
                corner.lat(),
                corner.lng(),
            );
            assert!(
                pt.z >= xzbbox.min_z() && pt.z <= xzbbox.max_z(),
                "z={} out of xzbbox [{}, {}] for corner ({}, {})",
                pt.z,
                xzbbox.min_z(),
                xzbbox.max_z(),
                corner.lat(),
                corner.lng(),
            );
        }
    }

    #[test]
    fn test_with_projection_matches_standalone_projection() {
        // Verify that CoordTransformer in WebMercator mode produces the same
        // result as calling WebMercatorProjection::forward directly.
        let llbbox = get_llbbox_arnis();
        let origin_lat = (llbbox.min().lat() + llbbox.max().lat()) / 2.0;
        let origin_lon = (llbbox.min().lng() + llbbox.max().lng()) / 2.0;
        let proj = crate::projection::WebMercatorProjection::new(origin_lat, origin_lon, 1.0);
        // A second, identically-parameterized instance for the direct-call
        // comparison below, since `with_projection` takes ownership of `proj`.
        let proj_direct = crate::projection::WebMercatorProjection::new(origin_lat, origin_lon, 1.0);
        let (transformer, _) =
            CoordTransformer::with_projection(&llbbox, 1.0, Box::new(proj)).unwrap();

        let test_point = LLPoint::new(
            llbbox.min().lat() + (llbbox.max().lat() - llbbox.min().lat()) * 0.3,
            llbbox.min().lng() + (llbbox.max().lng() - llbbox.min().lng()) * 0.7,
        )
        .unwrap();

        let pt = transformer.transform_point(test_point);
        let (expected_x, expected_z) = crate::projection::Projection::forward(
            &proj_direct,
            test_point.lat(),
            test_point.lng(),
        );

        // Integer truncation: the transformer casts with `as i32`
        assert_eq!(pt.x, expected_x as i32);
        assert_eq!(pt.z, expected_z as i32);
    }

    #[test]
    fn test_with_projection_east_increases_x() {
        let llbbox = get_llbbox_arnis();
        let proj = crate::projection::WebMercatorProjection::new(54.63, 9.93, 1.0);
        let (transformer, _) =
            CoordTransformer::with_projection(&llbbox, 1.0, Box::new(proj)).unwrap();

        let west = LLPoint::new(54.63, 9.928).unwrap();
        let east = LLPoint::new(54.63, 9.937).unwrap();

        let pw = transformer.transform_point(west);
        let pe = transformer.transform_point(east);
        assert!(
            pe.x > pw.x,
            "east should have larger x: west.x={}, east.x={}",
            pw.x,
            pe.x,
        );
    }

    #[test]
    fn test_with_projection_north_decreases_z() {
        let llbbox = get_llbbox_arnis();
        let proj = crate::projection::WebMercatorProjection::new(54.63, 9.93, 1.0);
        let (transformer, _) =
            CoordTransformer::with_projection(&llbbox, 1.0, Box::new(proj)).unwrap();

        let south = LLPoint::new(54.628, 9.93).unwrap();
        let north = LLPoint::new(54.634, 9.93).unwrap();

        let ps = transformer.transform_point(south);
        let pn = transformer.transform_point(north);
        assert!(
            pn.z < ps.z,
            "north should have smaller z: south.z={}, north.z={}",
            ps.z,
            pn.z,
        );
    }

    // ----- Korea TM (EPSG:5186) projection mode tests -----
    //
    // A precise 1500m x 1500m square (SPEC_GenerationScope P0's "영선동 일대
    // 1.5km 사각형") centred on Yeongseon-dong, Busan (35.0835693N 129.0414405E,
    // OSM Nominatim). Built by projecting the center to EPSG:5186 (E,N),
    // offsetting +-750m in that metric space, and projecting back -- not a
    // naive +-0.0075 degree box, which isn't square in real metres this far
    // from the equator. Corners cross-checked against `proj4` (npm, same
    // EPSG:5186 proj-string as korea_tm.rs's own reference values):
    //   sw E,N = 385433.176, 277523.071 -> lat,lon 35.07695141877934, 129.03305391424703
    //   ne E,N = 386933.176, 279023.071 -> lat,lon 35.090186547431124, 129.04982843930924
    fn yeongseon_1_5km_square_llbbox() -> LLBBox {
        LLBBox::new(
            35.07695141877934,
            129.03305391424703,
            35.090186547431124,
            129.04982843930924,
        )
        .unwrap()
    }

    #[test]
    fn test_korea_tm_sw_corner_maps_near_origin() {
        let llbbox = yeongseon_1_5km_square_llbbox();
        let proj =
            crate::projection::KoreaTmProjection::new(llbbox.min().lat(), llbbox.min().lng(), 1.75);
        let (transformer, _) =
            CoordTransformer::with_projection(&llbbox, 1.75, Box::new(proj)).unwrap();

        let sw = LLPoint::new(llbbox.min().lat(), llbbox.min().lng()).unwrap();
        let pt = transformer.transform_point(sw);
        assert_eq!(pt.x, 0, "SW corner should map to x=0, got {}", pt.x);
        assert_eq!(pt.z, 0, "SW corner should map to z=0, got {}", pt.z);
    }

    #[test]
    fn test_korea_tm_east_increases_x_north_decreases_z() {
        let llbbox = yeongseon_1_5km_square_llbbox();
        let proj =
            crate::projection::KoreaTmProjection::new(llbbox.min().lat(), llbbox.min().lng(), 1.75);
        let (transformer, _) =
            CoordTransformer::with_projection(&llbbox, 1.75, Box::new(proj)).unwrap();

        let sw = LLPoint::new(llbbox.min().lat(), llbbox.min().lng()).unwrap();
        let ne = LLPoint::new(llbbox.max().lat(), llbbox.max().lng()).unwrap();
        let p_sw = transformer.transform_point(sw);
        let p_ne = transformer.transform_point(ne);

        assert!(p_ne.x > p_sw.x, "east should increase x");
        assert!(p_ne.z < p_sw.z, "north should decrease z");
    }

    #[test]
    fn test_korea_tm_with_projection_envelope_is_wider_than_nominal() {
        // `with_projection` (the generic, four-corner-envelope path) gives
        // NOT 2625 (1500m * 1.75), even though the bbox is exactly 1500m
        // SW-to-NE by construction (see the fixture above). Yeongseon-dong
        // sits ~2 degrees east of EPSG:5186's central meridian (127E) -- far
        // enough for grid convergence to matter. A lat/lon rectangle is not a
        // square in E/N space that far from the central meridian: its NW/SE
        // corners land outside the SW-NE diagonal's own bounding box (project
        // them and see -- NW.x comes out negative, SE.x comes out past NE.x),
        // so the envelope is measurably wider than the nominal 1500m. This is
        // correct TM behaviour for this constructor, not a bug -- see
        // `test_korea_tm_from_planar_extents_gives_the_exact_square` below
        // for the constructor that does NOT have this widening, which is
        // what `--input-source kr` actually uses now.
        //
        // Expected values cross-checked independently via `proj4` (npm),
        // projecting all four corners with the exact same E0/N0-shift-then-
        // scale steps `with_projection`/`KoreaTmProjection::forward` use:
        //   nw=(-52.4,-2570.1) se=(2677.9,-54.9) ne=(2625.0,-2625.0) sw=(0,0)
        //   -> x in [-53, 2678] (2731 wide), z in [-2626, 0] (2626 wide)
        let llbbox = yeongseon_1_5km_square_llbbox();
        let proj =
            crate::projection::KoreaTmProjection::new(llbbox.min().lat(), llbbox.min().lng(), 1.75);
        let (transformer, xzbbox) =
            CoordTransformer::with_projection(&llbbox, 1.75, Box::new(proj)).unwrap();

        assert!(
            (transformer.scale_factor_x() - 2731.0).abs() < 2.0,
            "unexpected x extent in blocks: {}",
            transformer.scale_factor_x()
        );
        assert!(
            (transformer.scale_factor_z() - 2626.0).abs() < 2.0,
            "unexpected z extent in blocks: {}",
            transformer.scale_factor_z()
        );
        assert!(xzbbox.max_x() - xzbbox.min_x() > 2000);
    }

    #[test]
    fn test_korea_tm_from_planar_extents_does_not_further_distort_the_llbbox_envelope() {
        // `from_llbbox`'s envelope is genuinely ~1560m x 1500m, not 1500x1500
        // (see KoreaPlanarBBox's own width/height tests) -- a lat/lon
        // rectangle really does cover a non-square area in E/N this far from
        // the central meridian, and that is not something to "fix away" when
        // converting FROM lat/lon; it is what that lat/lon rectangle covers.
        // What `from_planar_extents` fixes is that this rectangle, once
        // known, is reproduced exactly -- not widened AGAIN the way
        // `with_projection`'s independent 4-corner re-projection would.
        let llbbox = yeongseon_1_5km_square_llbbox();
        let planar = crate::projection::korea_tm::KoreaPlanarBBox::from_llbbox(&llbbox);
        let proj = crate::projection::KoreaTmProjection::with_origin_en(
            planar.e_min(),
            planar.n_min(),
            1.75,
        );
        let width_blocks = (planar.width_m() * 1.75).round() as i32;
        let height_blocks = (planar.height_m() * 1.75).round() as i32;
        let (transformer, xzbbox) =
            CoordTransformer::from_planar_extents(Box::new(proj), width_blocks, height_blocks)
                .unwrap();

        assert_eq!(transformer.scale_factor_x() as i32, width_blocks);
        assert_eq!(transformer.scale_factor_z() as i32, height_blocks);
        assert_eq!(xzbbox.min_x(), 0);
        assert_eq!(xzbbox.min_z(), -height_blocks);
        assert_eq!(xzbbox.max_x(), width_blocks);
        assert_eq!(xzbbox.max_z(), 0);
        assert!((width_blocks - 2731).abs() <= 1, "width_blocks={width_blocks}");
        assert!((height_blocks - 2625).abs() <= 1, "height_blocks={height_blocks}");
    }

    #[test]
    fn test_korea_tm_from_planar_extents_with_direct_en_gives_a_true_square() {
        // The actual fix for "I want an exact 1500m square": specify E/N
        // directly (KoreaPlanarBBox::new, what `--bbox-en` parses to) instead
        // of deriving it from a lat/lon rectangle at all. No lat/lon
        // rectangle is ever built or projected, so there is no shear to
        // widen anything -- width and height in blocks come out identically
        // (up to the same rounding on each side), unlike the llbbox-derived
        // case above.
        let planar = crate::projection::korea_tm::KoreaPlanarBBox::new(
            385_433.1762, 277_523.0711, 386_933.1762, 279_023.0711,
        )
        .unwrap();
        assert!((planar.width_m() - 1500.0).abs() < 1.0e-3);
        assert!((planar.height_m() - 1500.0).abs() < 1.0e-3);

        let proj = crate::projection::KoreaTmProjection::with_origin_en(
            planar.e_min(),
            planar.n_min(),
            1.75,
        );
        let width_blocks = (planar.width_m() * 1.75).round() as i32;
        let height_blocks = (planar.height_m() * 1.75).round() as i32;
        let (_, xzbbox) =
            CoordTransformer::from_planar_extents(Box::new(proj), width_blocks, height_blocks)
                .unwrap();

        assert_eq!(width_blocks, 2625);
        assert_eq!(height_blocks, 2625);
        assert_eq!(xzbbox.max_x() - xzbbox.min_x(), xzbbox.max_z() - xzbbox.min_z());
    }

    #[test]
    fn test_local_mode_unaffected_by_projection_addition() {
        // Double-check that the Local path is bit-identical to pre-change behavior.
        let llbbox = get_llbbox_arnis();
        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(&llbbox, 1.0).unwrap();

        let (scale_factor_z, scale_factor_x) = geo_distance(llbbox.min(), llbbox.max());
        let scale_factor_z = scale_factor_z.floor();
        let scale_factor_x = scale_factor_x.floor();

        let llpoint = LLPoint::new(
            llbbox.min().lat() + (llbbox.max().lat() - llbbox.min().lat()) * 0.5,
            llbbox.min().lng() + (llbbox.max().lng() - llbbox.min().lng()) * 0.5,
        )
        .unwrap();

        let (expected_x, expected_z) = lat_lon_to_minecraft_coords(
            llpoint.lat(),
            llpoint.lng(),
            llbbox,
            scale_factor_z,
            scale_factor_x,
        );

        let pt = transformer.transform_point(llpoint);
        assert_eq!(pt.x, expected_x);
        assert_eq!(pt.z, expected_z);
    }
}
