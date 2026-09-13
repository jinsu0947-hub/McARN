//! SPEC_Build.md M2 "정류소 좌표 배치"의 입력 단계: 부산 버스 정류소 SHP
//! (`부산광역시_버스 정류소 정보(SHP)_20250121/tl_bus_station_info.*`)를
//! 적재하고, `.prj`가 선언한 좌표계(`GCS_WGS_1984`, 위경도 그대로 -- MOCT
//! 표준노드링크의 EPSG:5186 투영좌표와 다르다)에서 `korea_tm` 투영을 거쳐
//! 블록 좌표로 바꾼다.
//!
//! **여기서는 어떤 정류소가 최종 생성 대상인지 정하지 않는다.** "대상 정류소는
//! 구 경계가 아니라 대상 노선의 경유 여부로 정해진다"는 지시에 따라, 행정구역
//! 클립도 노선 필터도 하지 않고 원본 SHP의 전 레코드(부산광역시 전체 8522건)를
//! `bstopid`로 키를 삼아 그대로 싣는다. 스코핑(508 및 영도 경유 노선이 실제로
//! 서는 정류소만 추림)은 노선-정류소 매핑을 조인한 뒤의 몫이다 -- 그리고 그
//! 조인은 아직 성립하지 않는다: `crate::kr_bus_routes`의 모듈 문서 참고.

pub mod matching;

use crate::kr_roads::shapefile::{self, DbfTable};
use crate::kr_roads::en_to_block;
use crate::projection::korea_tm::{KoreaPlanarBBox, KoreaTmProjection};
use std::collections::HashMap;
use std::path::Path;

/// One 부산 버스 정류소 SHP record, position already projected to this run's
/// block coordinates.
pub struct BusStop {
    pub bstopid: u64,
    /// ARS 번호(정류소 표지판에 적힌 번호). 다수 레코드, 특히 마을버스 정류소가
    /// 공란("---")이다 -- 값이 있다는 보장은 없으니 빈 문자열/플레이스홀더
    /// 취급은 호출부 몫으로 남긴다.
    pub ars_no: String,
    pub name: String,
    /// 원본 값 그대로("일반" | "마을") -- 이 필드의 의미를 정의하는 SPEC이
    /// 없으므로 닫힌 enum으로 바꾸지 않는다.
    pub stop_type: String,
    pub lat: f64,
    pub lon: f64,
    pub x: i32,
    pub z: i32,
}

fn verify_fields(dbf: &DbfTable) -> Result<(), String> {
    let required = ["bstopid", "bstopnm", "stoptype", "arsno"];
    let fields = dbf.field_names();
    for f in required {
        if !fields.contains(&f) {
            return Err(format!(
                "bus stop .dbf is missing field {f} (실측: 필드 arsno/bstopid/bstopnm/gpsx/gpsy/stoptype 확인됨). Actual fields: {fields:?}"
            ));
        }
    }
    Ok(())
}

/// Loads every record from `shp_dir` (expects the 배포본 파일명
/// `tl_bus_station_info.shp/.dbf/.shx` grouped there), projects each into
/// this run's block coordinates, and returns them keyed by `bstopid` -- the
/// key a future route-stop join will look up by. No filtering: see the
/// module doc for why.
pub fn load_bus_stops(
    shp_dir: &Path,
    planar: &KoreaPlanarBBox,
    scale: f64,
) -> Result<HashMap<u64, BusStop>, String> {
    let base = shp_dir.join("tl_bus_station_info");
    let dbf = DbfTable::open(&base.with_extension("dbf"))
        .map_err(|e| format!("tl_bus_station_info.dbf: {e}"))?;
    verify_fields(&dbf)?;
    let points = shapefile::read_points(&base.with_extension("shp"))
        .map_err(|e| format!("tl_bus_station_info.shp: {e}"))?;
    shapefile::assert_row_counts_match(&dbf, points.len(), "tl_bus_station_info").map_err(|e| e.to_string())?;

    let mut stops = HashMap::with_capacity(points.len());
    let mut unparsed_ids = 0usize;
    for (i, &(lon, lat)) in points.iter().enumerate() {
        let bstopid_text = dbf.get(i, "bstopid");
        let Ok(bstopid) = bstopid_text.parse::<u64>() else {
            unparsed_ids += 1;
            continue;
        };
        // Shapefile geometry is (X, Y) = (lon, lat) here -- confirmed against
        // this file's own `gpsx`/`gpsy` dbf fields, which carry the same
        // values (see the loading-session investigation).
        let (e, n) = KoreaTmProjection::project_raw(lat, lon);
        let (bx, bz) = en_to_block(e, n, planar, scale);
        stops.insert(
            bstopid,
            BusStop {
                bstopid,
                ars_no: dbf.get(i, "arsno"),
                name: dbf.get(i, "bstopnm"),
                stop_type: dbf.get(i, "stoptype"),
                lat,
                lon,
                x: bx.round() as i32,
                z: bz.round() as i32,
            },
        );
    }
    if unparsed_ids > 0 {
        eprintln!(
            "Warning: bus stops: {unparsed_ids} record(s) had a non-numeric bstopid and were skipped"
        );
    }
    Ok(stops)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_a_known_point_toward_the_planar_origin() {
        // 영주삼거리 (부산 중구), record 1 of the real SHP: lon 129.0332616667,
        // lat 35.1153233333. Reference point is projected raw, so this checks
        // the pipeline end-to-end without needing a real KoreaPlanarBBox.
        let (e, n) = KoreaTmProjection::project_raw(35.1153233333, 129.0332616667);
        let planar = KoreaPlanarBBox::new(e - 100.0, n - 100.0, e + 100.0, n + 100.0).unwrap();
        let (bx, bz) = en_to_block(e, n, &planar, 1.75);
        // The point is the bbox's centre by construction: 100m in from every
        // edge, at scale 1.75 blocks/m -> 175 blocks from the x=0 edge, and
        // -175 from the z=0 edge (north = -Z, per `en_to_block`'s own doc).
        assert!((bx - 175.0).abs() < 1.0, "bx={bx}");
        assert!((bz - -175.0).abs() < 1.0, "bz={bz}");
    }
}
