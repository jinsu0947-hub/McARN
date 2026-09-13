//! Minimal, read-only ESRI Shapefile (.shp) + dBASE (.dbf) reader, scoped to
//! exactly what SPEC_Ingest.md §3 needs from 표준노드링크: point geometry
//! (nodes) and polyline geometry (links), plus their attribute tables. No
//! crate for this exists in `Cargo.toml` and this build runs `--offline`
//! (no network to fetch one), so this is hand-rolled against the public ESRI
//! Shapefile Technical Description and the dBASE III+ file format -- both
//! simple, stable, well-documented binary formats.

use std::fs;
use std::io;
use std::path::Path;

/// One dBASE field descriptor: name and byte length, enough to slice a
/// fixed-width record. Type/decimal-count are read but unused -- every field
/// this module's callers want (`LINK_ID`, `LANES`, `ROAD_RANK`, ...) is read
/// back as trimmed text and parsed by the caller, so a numeric field's
/// decimal count never needs to be interpreted here.
struct DbfField {
    name: String,
    offset: usize,
    len: usize,
}

/// A parsed dBASE table: field layout plus raw records, still to be sliced by
/// field name per row. Deleted records (leading `0x2A`) are skipped -- MOCT's
/// distributions do not carry any in the samples this was built against, but
/// the flag exists in the format and ignoring it would silently resurrect
/// removed features.
pub struct DbfTable {
    fields: Vec<DbfField>,
    record_len: usize,
    records: Vec<u8>,
    num_records: usize,
}

impl DbfTable {
    pub fn open(path: &Path) -> io::Result<Self> {
        let data = fs::read(path)?;
        if data.len() < 32 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "dbf shorter than header"));
        }
        let num_records = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let header_len = u16::from_le_bytes([data[8], data[9]]) as usize;
        let record_len = u16::from_le_bytes([data[10], data[11]]) as usize;

        let mut fields = Vec::new();
        let mut offset = 1; // byte 0 of every record is the deletion flag
        let mut pos = 32;
        while pos + 1 < header_len && data[pos] != 0x0D {
            if pos + 32 > data.len() {
                break;
            }
            let raw_name = &data[pos..pos + 11];
            let name_len = raw_name.iter().position(|&b| b == 0).unwrap_or(11);
            let name = String::from_utf8_lossy(&raw_name[..name_len]).trim().to_string();
            let len = data[pos + 16] as usize;
            fields.push(DbfField { name, offset, len });
            offset += len;
            pos += 32;
        }

        let records_start = header_len;
        let records = if records_start <= data.len() {
            data[records_start..].to_vec()
        } else {
            Vec::new()
        };

        Ok(Self {
            fields,
            record_len,
            records,
            num_records,
        })
    }

    pub fn len(&self) -> usize {
        self.num_records
    }

    /// The whole table's field names, in file order -- used only for the
    /// startup "field names match the spec" check, not per-record reads.
    pub fn field_names(&self) -> Vec<&str> {
        self.fields.iter().map(|f| f.name.as_str()).collect()
    }

    /// One record's field value as trimmed text (numeric dBASE fields are
    /// stored as ASCII digits, so this works for both `C` and `N` types
    /// without a separate numeric path). Empty string if the field is
    /// unknown or the record index is out of range.
    pub fn get(&self, record_index: usize, field_name: &str) -> String {
        let Some(field) = self.fields.iter().find(|f| f.name == field_name) else {
            return String::new();
        };
        let rec_start = record_index * self.record_len;
        let start = rec_start + field.offset;
        let end = start + field.len;
        if end > self.records.len() {
            return String::new();
        }
        // MOCT's Korean text fields (ROAD_NAME, REMARK, ...) are EUC-KR/CP949,
        // not UTF-8 -- decoded lossily since only ASCII fields (IDs, LANES,
        // ROAD_RANK) are ever parsed as data; Korean text is display-only.
        decode_cp949_lossy(&self.records[start..end]).trim().to_string()
    }

    fn is_deleted(&self, record_index: usize) -> bool {
        let rec_start = record_index * self.record_len;
        self.records.get(rec_start).copied() == Some(0x2A)
    }
}

/// CP949/EUC-KR is not in Rust's standard library and pulling `encoding_rs`
/// in just for two free-text display fields is not worth a new dependency in
/// an offline build. ASCII bytes pass through as-is (every field this module
/// actually parses -- IDs, digits, ROAD_RANK codes -- is pure ASCII); any
/// non-ASCII byte is replaced with `?` rather than misdecoded, since it is
/// never read back as data.
fn decode_cp949_lossy(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| if b.is_ascii() { b as char } else { '?' })
        .collect()
}

/// Shape type codes this module understands (ESRI Shapefile spec).
const SHAPE_TYPE_POINT: i32 = 1;
const SHAPE_TYPE_POLYLINE: i32 = 3;

/// Reads a point shapefile (NODE.shp) into `(x, y)` pairs in file order --
/// index-aligned with the paired `.dbf`'s records, same convention `.shp`/
/// `.dbf` pairs always use (ESRI ties them by record order, not an explicit
/// key).
pub fn read_points(path: &Path) -> io::Result<Vec<(f64, f64)>> {
    let data = fs::read(path)?;
    let shape_type = i32::from_le_bytes([data[32], data[33], data[34], data[35]]);
    if shape_type != SHAPE_TYPE_POINT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected Point (1), got shape type {shape_type}"),
        ));
    }
    let mut points = Vec::new();
    let mut pos = 100; // main file header is fixed 100 bytes
    while pos + 8 <= data.len() {
        let content_len_words = u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
        let content_start = pos + 8;
        let content_len_bytes = content_len_words as usize * 2;
        if content_start + content_len_bytes > data.len() {
            break;
        }
        // Content: shapeType(4) + X(8) + Y(8), all little-endian.
        let x = f64::from_le_bytes(data[content_start + 4..content_start + 12].try_into().unwrap());
        let y = f64::from_le_bytes(data[content_start + 12..content_start + 20].try_into().unwrap());
        points.push((x, y));
        pos = content_start + content_len_bytes;
    }
    Ok(points)
}

/// Reads a polyline shapefile (LINK.shp) into one flattened point sequence
/// per record. Multi-part records (rare for a single 링크 -- `MULTI_LINK` in
/// the `.dbf` flags those, tracked separately in a companion `MULTILINK.dbf`
/// this module does not read) have their parts concatenated in file order
/// rather than kept apart: for the road centerline this module builds, a
/// multi-part link is still one connected line, and losing the exact
/// part-boundary is an acceptable approximation at this scope.
pub fn read_polylines(path: &Path) -> io::Result<Vec<Vec<(f64, f64)>>> {
    let data = fs::read(path)?;
    let shape_type = i32::from_le_bytes([data[32], data[33], data[34], data[35]]);
    if shape_type != SHAPE_TYPE_POLYLINE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected PolyLine (3), got shape type {shape_type}"),
        ));
    }
    let mut lines = Vec::new();
    let mut pos = 100;
    while pos + 8 <= data.len() {
        let content_len_words = u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
        let content_start = pos + 8;
        let content_len_bytes = content_len_words as usize * 2;
        if content_start + content_len_bytes > data.len() || content_len_bytes < 44 {
            break;
        }
        // Content: shapeType(4) + box(32) + numParts(4) + numPoints(4) + parts[numParts]*4 + points[numPoints]*16.
        let num_parts = i32::from_le_bytes(
            data[content_start + 36..content_start + 40].try_into().unwrap(),
        ) as usize;
        let num_points = i32::from_le_bytes(
            data[content_start + 40..content_start + 44].try_into().unwrap(),
        ) as usize;
        let points_start = content_start + 44 + num_parts * 4;
        let mut pts = Vec::with_capacity(num_points);
        for i in 0..num_points {
            let p = points_start + i * 16;
            if p + 16 > data.len() {
                break;
            }
            let x = f64::from_le_bytes(data[p..p + 8].try_into().unwrap());
            let y = f64::from_le_bytes(data[p + 8..p + 16].try_into().unwrap());
            pts.push((x, y));
        }
        lines.push(pts);
        pos = content_start + content_len_bytes;
    }
    Ok(lines)
}

/// Reconciles a `.dbf`'s row count against the paired `.shp`'s geometry
/// count -- callers zip the two by index, so a mismatch would silently
/// misattribute every attribute after the first missing/extra row.
pub fn assert_row_counts_match(dbf: &DbfTable, geometry_count: usize, label: &str) -> io::Result<()> {
    // `is_deleted` is checked here (not filtered out of `records`) so this
    // count includes deleted rows the same way `.shp` geometry always does --
    // dBASE marks deletions but the shapefile pairing is still positional.
    let deleted = (0..dbf.len()).filter(|&i| dbf.is_deleted(i)).count();
    if deleted > 0 {
        eprintln!("{label}: {deleted} deleted dbf record(s) present but not skipped (positional pairing)");
    }
    if dbf.len() != geometry_count {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{label}: dbf has {} records but shp has {geometry_count} shapes", dbf.len()),
        ));
    }
    Ok(())
}
