//! Terrain computation cache: `Ground::new_enabled`'s fetched-and-repaired
//! elevation/land-cover/canopy grids, saved keyed by every input that
//! determines them, so a repeat run with the same key loads instead of
//! re-fetching and re-repairing. Measured motivation (a real 7x4km run):
//! land-cover repair (Gaussian smoothing, coastal/shore correction) alone
//! took ~60% of total wall time -- far more than the raw tile fetch itself,
//! so caching only the *raw* downloaded tiles (which `elevation::cache`/
//! `land_cover` already do) leaves the actual bottleneck untouched. This
//! cache stores the grid *after* every repair pass, so a hit skips all of
//! it.
//!
//! **This is a computation-result cache, not a world.** It is a flat binary
//! file of plain grids (heights, land-cover classes, canopy heights) plus a
//! header recording exactly what run produced them. It never constructs a
//! `WorldEditor` and never reads `.mca`/`level.dat` -- see `kr_roads`'s own
//! module doc for why reopening a *saved world* to reuse its terrain
//! corrupted it the first time this project tried that shortcut. A
//! computation cache and a world are not the same kind of file, and this
//! module only ever touches the former.
//!
//! Hand-rolled binary format, the same call `kr_roads::shapefile` made:
//! no serialization crate is already in this project's vetted dependency
//! set, this build runs `--offline`, and the actual shape here (a handful
//! of numeric grids plus a small scalar header) doesn't need a general
//! framework.

use crate::celestial::CelestialBody;
use crate::climate::Climate;
use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::postprocess::ElevationCompressionInfo;
use crate::elevation::ElevationData;
use crate::ground::Ground;
use crate::land_cover::LandCoverData;
use crate::canopy::CanopyData;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Bumped whenever this binary layout changes, or whenever anything about
/// how `Ground::new_enabled` computes its result changes -- a cache written
/// by an older/different version of this logic must never be silently
/// reused just because the run parameters happen to match.
const CACHE_FORMAT_VERSION: u32 = 1;
const MAGIC: u32 = 0x54_43_41_43; // "TCAC"

/// Every input that determines `Ground::new_enabled`'s output. `benchmark`
/// is deliberately absent -- it only toggles logging, not the result.
#[derive(Clone, Copy)]
pub struct CacheKey {
    pub bbox: LLBBox,
    pub scale: f64,
    pub ground_level: i32,
    pub min_ground_level: i32,
    pub disable_height_limit: bool,
    pub extended_max_y: i32,
    pub aws_only_elevation: bool,
    pub canopy_height: bool,
    pub body: CelestialBody,
}

impl CacheKey {
    /// Short, filename-safe identifier. Not the sole check -- `load` still
    /// compares the full key byte-for-byte against what's stored in the
    /// file before trusting a hit, so a collision here can only ever cause
    /// an extra cache miss, never a wrong hit.
    fn file_stem(&self) -> String {
        let mut h: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
        let mut mix = |bytes: &[u8]| {
            for &b in bytes {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
        };
        mix(&self.bbox.min().lat().to_le_bytes());
        mix(&self.bbox.min().lng().to_le_bytes());
        mix(&self.bbox.max().lat().to_le_bytes());
        mix(&self.bbox.max().lng().to_le_bytes());
        mix(&self.scale.to_le_bytes());
        mix(&self.ground_level.to_le_bytes());
        mix(&self.min_ground_level.to_le_bytes());
        mix(&[self.disable_height_limit as u8]);
        mix(&self.extended_max_y.to_le_bytes());
        mix(&[self.aws_only_elevation as u8]);
        mix(&[self.canopy_height as u8]);
        mix(&[body_tag(self.body)]);
        format!("terrain_{h:016x}")
    }

    fn matches(&self, other: &CacheKey) -> bool {
        self.bbox == other.bbox
            && self.scale.to_bits() == other.scale.to_bits()
            && self.ground_level == other.ground_level
            && self.min_ground_level == other.min_ground_level
            && self.disable_height_limit == other.disable_height_limit
            && self.extended_max_y == other.extended_max_y
            && self.aws_only_elevation == other.aws_only_elevation
            && self.canopy_height == other.canopy_height
            && body_tag(self.body) == body_tag(other.body)
    }
}

fn body_tag(b: CelestialBody) -> u8 {
    match b {
        CelestialBody::Earth => 0,
        CelestialBody::Moon => 1,
        CelestialBody::Mars => 2,
    }
}

fn body_from_tag(t: u8) -> io::Result<CelestialBody> {
    match t {
        0 => Ok(CelestialBody::Earth),
        1 => Ok(CelestialBody::Moon),
        2 => Ok(CelestialBody::Mars),
        _ => Err(invalid_data(format!("unknown celestial body tag {t}"))),
    }
}

fn climate_tag(c: Climate) -> u8 {
    match c {
        Climate::Temperate => 0,
        Climate::TropicalSavanna => 1,
        Climate::HotDesert => 2,
        Climate::HotSteppe => 3,
        Climate::ColdDesert => 4,
        Climate::ColdSteppe => 5,
        Climate::DryContinental => 6,
        Climate::Boreal => 7,
        Climate::Tundra => 8,
        Climate::IceCap => 9,
    }
}

fn climate_from_tag(t: u8) -> io::Result<Climate> {
    Ok(match t {
        0 => Climate::Temperate,
        1 => Climate::TropicalSavanna,
        2 => Climate::HotDesert,
        3 => Climate::HotSteppe,
        4 => Climate::ColdDesert,
        5 => Climate::ColdSteppe,
        6 => Climate::DryContinental,
        7 => Climate::Boreal,
        8 => Climate::Tundra,
        9 => Climate::IceCap,
        _ => return Err(invalid_data(format!("unknown climate tag {t}"))),
    })
}

fn invalid_data(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

fn cache_path(dir: &Path, key: &CacheKey) -> PathBuf {
    dir.join(format!("{}.bin", key.file_stem()))
}

// --- primitive writers/readers -------------------------------------------

fn w_u8<W: Write>(w: &mut W, v: u8) -> io::Result<()> {
    w.write_all(&[v])
}
fn w_bool<W: Write>(w: &mut W, v: bool) -> io::Result<()> {
    w_u8(w, v as u8)
}
fn w_u32<W: Write>(w: &mut W, v: u32) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn w_u64<W: Write>(w: &mut W, v: u64) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn w_i32<W: Write>(w: &mut W, v: i32) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn w_f64<W: Write>(w: &mut W, v: f64) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn w_bytes<W: Write>(w: &mut W, v: &[u8]) -> io::Result<()> {
    w_u64(w, v.len() as u64)?;
    w.write_all(v)
}
fn w_str<W: Write>(w: &mut W, s: &str) -> io::Result<()> {
    w_bytes(w, s.as_bytes())
}
/// Bulk-writes an `f32` slice as raw little-endian-native bytes. Sound: any
/// 4-byte pattern is a valid `f32` (unlike, say, `bool` or an enum), so the
/// reinterpretation never produces an invalid value in either direction --
/// only ever used for this cache's own round-trip on the same machine.
fn w_f32_slice<W: Write>(w: &mut W, v: &[f32]) -> io::Result<()> {
    w_u64(w, v.len() as u64)?;
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), std::mem::size_of_val(v)) };
    w.write_all(bytes)
}
fn w_grid_u8<W: Write>(w: &mut W, grid: &[Vec<u8>]) -> io::Result<()> {
    w_u64(w, grid.len() as u64)?;
    for row in grid {
        w_bytes(w, row)?;
    }
    Ok(())
}
fn w_grid_f32<W: Write>(w: &mut W, grid: &[Vec<f32>]) -> io::Result<()> {
    w_u64(w, grid.len() as u64)?;
    for row in grid {
        w_f32_slice(w, row)?;
    }
    Ok(())
}

fn r_u8<R: Read>(r: &mut R) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}
fn r_bool<R: Read>(r: &mut R) -> io::Result<bool> {
    Ok(r_u8(r)? != 0)
}
fn r_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn r_u64<R: Read>(r: &mut R) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn r_i32<R: Read>(r: &mut R) -> io::Result<i32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_le_bytes(b))
}
fn r_f64<R: Read>(r: &mut R) -> io::Result<f64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(f64::from_le_bytes(b))
}
fn r_bytes<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let n = r_u64(r)? as usize;
    let mut v = vec![0u8; n];
    r.read_exact(&mut v)?;
    Ok(v)
}
fn r_str<R: Read>(r: &mut R) -> io::Result<String> {
    String::from_utf8(r_bytes(r)?).map_err(|e| invalid_data(e.to_string()))
}
/// Inverse of `w_f32_slice`.
fn r_f32_vec<R: Read>(r: &mut R) -> io::Result<Vec<f32>> {
    let n = r_u64(r)? as usize;
    let mut v = vec![0f32; n];
    let bytes: &mut [u8] = unsafe { std::slice::from_raw_parts_mut(v.as_mut_ptr().cast::<u8>(), n * 4) };
    r.read_exact(bytes)?;
    Ok(v)
}
fn r_grid_u8<R: Read>(r: &mut R) -> io::Result<Vec<Vec<u8>>> {
    let n = r_u64(r)? as usize;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(r_bytes(r)?);
    }
    Ok(v)
}
fn r_grid_f32<R: Read>(r: &mut R) -> io::Result<Vec<Vec<f32>>> {
    let n = r_u64(r)? as usize;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(r_f32_vec(r)?);
    }
    Ok(v)
}

fn write_key<W: Write>(w: &mut W, key: &CacheKey) -> io::Result<()> {
    w_f64(w, key.bbox.min().lat())?;
    w_f64(w, key.bbox.min().lng())?;
    w_f64(w, key.bbox.max().lat())?;
    w_f64(w, key.bbox.max().lng())?;
    w_f64(w, key.scale)?;
    w_i32(w, key.ground_level)?;
    w_i32(w, key.min_ground_level)?;
    w_bool(w, key.disable_height_limit)?;
    w_i32(w, key.extended_max_y)?;
    w_bool(w, key.aws_only_elevation)?;
    w_bool(w, key.canopy_height)?;
    w_u8(w, body_tag(key.body))
}

fn read_key<R: Read>(r: &mut R) -> io::Result<CacheKey> {
    let min_lat = r_f64(r)?;
    let min_lng = r_f64(r)?;
    let max_lat = r_f64(r)?;
    let max_lng = r_f64(r)?;
    let bbox = LLBBox::new(min_lat, min_lng, max_lat, max_lng).map_err(invalid_data)?;
    Ok(CacheKey {
        bbox,
        scale: r_f64(r)?,
        ground_level: r_i32(r)?,
        min_ground_level: r_i32(r)?,
        disable_height_limit: r_bool(r)?,
        extended_max_y: r_i32(r)?,
        aws_only_elevation: r_bool(r)?,
        canopy_height: r_bool(r)?,
        body: body_from_tag(r_u8(r)?)?,
    })
}

fn write_elevation<W: Write>(w: &mut W, ed: &ElevationData) -> io::Result<()> {
    w_grid_f32(w, &ed.heights)?;
    w_u64(w, ed.width as u64)?;
    w_u64(w, ed.height as u64)?;
    w_u64(w, ed.world_width as u64)?;
    w_u64(w, ed.world_height as u64)?;
    w_f64(w, ed.min_height_m)?;
    w_f64(w, ed.blocks_per_meter)?;
    w_f64(w, ed.slope_correction)?;
    w_i32(w, ed.ground_level)?;
    w_f64(w, ed.elevation_mapping.h_linear_m)?;
    w_f64(w, ed.elevation_mapping.compression)?;
    w_f64(w, ed.elevation_mapping.max_source_elevation_m)
}

fn read_elevation<R: Read>(r: &mut R) -> io::Result<ElevationData> {
    let heights = r_grid_f32(r)?;
    let width = r_u64(r)? as usize;
    let height = r_u64(r)? as usize;
    let world_width = r_u64(r)? as usize;
    let world_height = r_u64(r)? as usize;
    let min_height_m = r_f64(r)?;
    let blocks_per_meter = r_f64(r)?;
    let slope_correction = r_f64(r)?;
    let ground_level = r_i32(r)?;
    let h_linear_m = r_f64(r)?;
    let compression = r_f64(r)?;
    let max_source_elevation_m = r_f64(r)?;
    Ok(ElevationData {
        heights,
        width,
        height,
        world_width,
        world_height,
        min_height_m,
        blocks_per_meter,
        slope_correction,
        ground_level,
        elevation_mapping: ElevationCompressionInfo { h_linear_m, compression, max_source_elevation_m },
    })
}

fn write_land_cover<W: Write>(w: &mut W, lc: &LandCoverData) -> io::Result<()> {
    w_grid_u8(w, &lc.grid)?;
    w_grid_u8(w, &lc.water_distance)?;
    w_u64(w, lc.width as u64)?;
    w_u64(w, lc.height as u64)?;
    w_f64(w, lc.cells_per_meter)
}

fn read_land_cover<R: Read>(r: &mut R) -> io::Result<LandCoverData> {
    let grid = r_grid_u8(r)?;
    let water_distance = r_grid_u8(r)?;
    let width = r_u64(r)? as usize;
    let height = r_u64(r)? as usize;
    let cells_per_meter = r_f64(r)?;
    Ok(LandCoverData {
        grid,
        water_distance,
        water_blend_cache: once_cell::sync::OnceCell::new(),
        width,
        height,
        cells_per_meter,
    })
}

/// Tries to load a cache hit for `key` from `dir`. `Ok(None)` is a plain
/// miss (no file for this key); `Err` is an existing file that failed to
/// parse (corrupt/truncated/foreign) -- both are treated the same way by
/// the caller (fall back to computing fresh), but are logged differently.
pub fn load(dir: &Path, key: &CacheKey) -> Result<Option<Ground>, String> {
    let path = cache_path(dir, key);
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut r = io::Cursor::new(data);
    (|| -> io::Result<Option<Ground>> {
        if r_u32(&mut r)? != MAGIC {
            return Err(invalid_data("bad magic".to_string()));
        }
        if r_u32(&mut r)? != CACHE_FORMAT_VERSION {
            // Not corrupt, just an older/newer layout -- a plain miss, not an error.
            return Ok(None);
        }
        let _crate_version = r_str(&mut r)?;
        let stored_key = read_key(&mut r)?;
        if !stored_key.matches(key) {
            // Hash collision between two different keys -- vanishingly
            // unlikely, but a plain miss, not a hard error, either way.
            return Ok(None);
        }
        let elevation_enabled = r_bool(&mut r)?;
        let extended_ceiling = r_bool(&mut r)?;
        let ground_level = r_i32(&mut r)?;
        let elevation_data = if r_bool(&mut r)? { Some(read_elevation(&mut r)?) } else { None };
        let land_cover = if r_bool(&mut r)? { Some(read_land_cover(&mut r)?) } else { None };
        let canopy = if r_bool(&mut r)? {
            let grid = r_bytes(&mut r)?;
            let w = r_u64(&mut r)? as usize;
            let h = r_u64(&mut r)? as usize;
            Some(CanopyData::from_grid(grid, w, h))
        } else {
            None
        };
        let world_width = r_u64(&mut r)? as usize;
        let world_height = r_u64(&mut r)? as usize;
        let snow_threshold_y = r_i32(&mut r)?;
        let climate = climate_from_tag(r_u8(&mut r)?)?;
        let body = body_from_tag(r_u8(&mut r)?)?;

        Ok(Some(Ground::from_cache_parts(
            elevation_enabled,
            extended_ceiling,
            ground_level,
            elevation_data,
            land_cover,
            canopy,
            world_width,
            world_height,
            snow_threshold_y,
            climate,
            body,
        )))
    })()
    .map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes `ground` to `dir` under `key`. Best-effort: a write failure (e.g.
/// a read-only cache dir) is reported to the caller as an error string to
/// log, not a hard failure of the run -- the terrain was already computed
/// and placed either way.
pub fn store(dir: &Path, key: &CacheKey, ground: &Ground) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = cache_path(dir, key);
    let tmp_path = path.with_extension("bin.tmp");
    let result = (|| -> io::Result<()> {
        let mut w = io::BufWriter::new(std::fs::File::create(&tmp_path)?);
        w_u32(&mut w, MAGIC)?;
        w_u32(&mut w, CACHE_FORMAT_VERSION)?;
        w_str(&mut w, env!("CARGO_PKG_VERSION"))?;
        write_key(&mut w, key)?;

        w_bool(&mut w, ground.elevation_enabled)?;
        w_bool(&mut w, ground.extended_ceiling())?;
        w_i32(&mut w, ground.base_level())?;

        if let Some(ed) = ground.elevation_data() {
            w_bool(&mut w, true)?;
            write_elevation(&mut w, ed)?;
        } else {
            w_bool(&mut w, false)?;
        }
        if let Some(lc) = ground.land_cover_data() {
            w_bool(&mut w, true)?;
            write_land_cover(&mut w, lc)?;
        } else {
            w_bool(&mut w, false)?;
        }
        if let Some(cd) = ground.canopy_data() {
            w_bool(&mut w, true)?;
            w_bytes(&mut w, cd.grid())?;
            w_u64(&mut w, cd.width as u64)?;
            w_u64(&mut w, cd.height as u64)?;
        } else {
            w_bool(&mut w, false)?;
        }
        let (world_width, world_height) = ground.world_dims();
        w_u64(&mut w, world_width as u64)?;
        w_u64(&mut w, world_height as u64)?;
        w_i32(&mut w, ground.snow_threshold_y())?;
        w_u8(&mut w, climate_tag(ground.climate()))?;
        w_u8(&mut w, body_tag(ground.body()))?;
        w.flush()
    })();

    match result {
        Ok(()) => {
            // Rename into place only after a fully successful write, so a
            // crash/interruption mid-write can never leave a truncated file
            // at the real cache path for a later run to (fail to) load.
            std::fs::rename(&tmp_path, &path).map_err(|e| e.to_string())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ground() -> (CacheKey, Ground) {
        let bbox = LLBBox::new(35.0, 129.0, 35.01, 129.01).unwrap();
        let key = CacheKey {
            bbox,
            scale: 1.75,
            ground_level: -62,
            min_ground_level: -88,
            disable_height_limit: false,
            extended_max_y: 319,
            aws_only_elevation: false,
            canopy_height: true,
            body: CelestialBody::Earth,
        };
        let elevation_data = ElevationData {
            heights: vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]],
            width: 3,
            height: 2,
            world_width: 3,
            world_height: 2,
            min_height_m: 0.0,
            blocks_per_meter: 1.75,
            slope_correction: 1.0,
            ground_level: -62,
            elevation_mapping: ElevationCompressionInfo { h_linear_m: 120.0, compression: 0.0, max_source_elevation_m: 50.0 },
        };
        let land_cover = LandCoverData {
            grid: vec![vec![10, 20, 30], vec![40, 50, 60]],
            water_distance: vec![vec![0, 0, 1], vec![0, 2, 3]],
            water_blend_cache: once_cell::sync::OnceCell::new(),
            width: 3,
            height: 2,
            cells_per_meter: 1.0,
        };
        let canopy = CanopyData::from_grid(vec![5, 6, 7, 8, 9, 10], 3, 2);
        let ground = Ground::from_cache_parts(
            true, false, -62, Some(elevation_data), Some(land_cover), Some(canopy), 3, 2, i32::MAX,
            Climate::Temperate, CelestialBody::Earth,
        );
        (key, ground)
    }

    #[test]
    fn stores_and_loads_an_identical_ground() {
        let dir = std::env::temp_dir().join(format!("terrain_cache_test_{}", std::process::id()));
        let (key, ground) = test_ground();
        store(&dir, &key, &ground).unwrap();
        let loaded = load(&dir, &key).unwrap().expect("should be a hit");

        assert_eq!(loaded.base_level(), ground.base_level());
        assert_eq!(loaded.elevation_enabled, ground.elevation_enabled);
        let (lw, lh) = loaded.world_dims();
        let (gw, gh) = ground.world_dims();
        assert_eq!((lw, lh), (gw, gh));
        assert_eq!(loaded.elevation_data().unwrap().heights, ground.elevation_data().unwrap().heights);
        assert_eq!(loaded.land_cover_data().unwrap().grid, ground.land_cover_data().unwrap().grid);
        assert_eq!(loaded.land_cover_data().unwrap().water_distance, ground.land_cover_data().unwrap().water_distance);
        assert_eq!(loaded.canopy_data().unwrap().grid(), ground.canopy_data().unwrap().grid());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_different_key_is_a_miss() {
        let dir = std::env::temp_dir().join(format!("terrain_cache_test_miss_{}", std::process::id()));
        let (mut key, ground) = test_ground();
        store(&dir, &key, &ground).unwrap();
        key.scale = 2.0; // different key -> different file_stem -> plain miss
        assert!(load(&dir, &key).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_is_a_plain_miss_not_an_error() {
        let dir = std::env::temp_dir().join(format!("terrain_cache_test_nofile_{}", std::process::id()));
        let (key, _ground) = test_ground();
        assert!(load(&dir, &key).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
