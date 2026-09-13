//! SPEC_Build.md M2 "버스 노선 폴리라인 구성"의 입력: 부산 버스노선별
//! 승하차 정보 CSV(`data/부산광역시_버스노선별 승하차 정보_20230731.csv`)를
//! 적재한다. 이 CSV는 CP949 인코딩이고, 앞 4열(노선번호/정류장순서/정류장코드/
//! 정류장명)만 쓴다 -- 나머지는 시간대별 승하차 통계로, 이 프로젝트가 읽을
//! 일이 없다.
//!
//! **기준일 2023-07-31** -- 파일명이 그대로 밝히는 값이다. SPEC_GenerationScope.md
//! §1.2("노선 데이터는 단일 기준일의 스냅샷으로 고정한다")에 따라 이 날짜를
//! `manifest.json`에 그대로 기록한다. 부산 시내버스 2025-07-05 전면개편
//! **이전** 자료라서, 개편 이후 신설·변경된 노선/정류소는 여기 없다 -- 지시에
//! 따라 이 사실을 감추지 않고 그대로 진행한다.
//!
//! **마을버스는 이 CSV에 없다.** 부산시내버스만 수록되어 있으므로, M2는
//! 시내버스 노선만으로 진행하고 마을버스는 별도 자료가 오면 붙인다.
//!
//! **`stop_code`는 아직 [`crate::kr_bus_stops::BusStop::bstopid`]와 조인할
//! 수 없다.** 실측 교차검증 결과:
//! - 이 CSV의 고유 `정류장코드` 4138개 중 SHP `bstopid`와 직접 일치 0건
//!   (`bstopid`는 9자리, `정류장코드`는 "26"(부산 시도코드로 보임) + 5자리
//!   형태로 자릿수부터 다르다).
//! - "26" + SHP의 `arsno`로 만든 값과는 784건(19%)이 우연히 겹치지만, 이름이
//!   명백히 같은 정류소로도 반례가 나온다 -- 예: 508번 3번째 정류소 "고신대학"의
//!   `정류장코드`는 2600394인데, SHP에서 이름이 같은 두 레코드의 `arsno`는
//!   04110/04111이라 "26"+arsno가 2604110/2604111이 되어 맞지 않는다.
//! - 두 자료가 서로 다른 발행 기관·시점의 별도 채번 체계를 쓴다는 뜻으로
//!   읽었다. 이름 기반 매칭은 방향별로 정류소명이 겹치는 경우(예: "고신대학"이
//!   SHP에 두 번, 왕복 양방향 각각)가 흔해 되짚을 수 없는 오배치를 조용히
//!   만들 위험이 있고, SPEC_Build.md §1은 정류소 위치 정확도를 "유일하게
//!   타협 불가한 항목"이라 명시한다 -- 그래서 이 모듈은 좌표를 붙이지 않고
//!   `stop_code`만 원문 그대로 들고 있는다. 조인 방법은 이 세션에서 사용자에게
//!   보고하고 확인을 기다리는 중이다.

use std::collections::HashMap;
use std::path::Path;

/// SPEC_GenerationScope.md §1.2's 기준일 -- this CSV's own filename date,
/// recorded verbatim rather than read from the file (the file has no
/// explicit as-of field of its own).
pub const REFERENCE_DATE: &str = "2023-07-31";

/// One CSV row's first four columns -- everything downstream (route
/// polyline construction) needs, once `stop_code` can be resolved to a
/// position.
pub struct RouteStop {
    pub route_no: String,
    pub seq: u32,
    pub stop_code: String,
    pub stop_name: String,
}

/// All route-stop rows, grouped by `route_no` and kept in the CSV's own
/// `정류장순서` order within each route (SPEC_GenerationScope.md §1.3: order
/// is what lets a route-network shortest-path fallback reconstruct the
/// polyline when link IDs aren't available -- they aren't, here).
pub struct RouteData {
    pub reference_date: &'static str,
    pub stops_by_route: HashMap<String, Vec<RouteStop>>,
}

/// Reads `csv_path` (CP949-encoded), keeping only the first four columns.
/// `encoding_rs::EUC_KR`'s decoder is the WHATWG Encoding Standard's
/// Windows-949 index -- a superset covering the same code points CP949
/// does, unlike Rust's standard library which has no CP949/EUC-KR decoder
/// at all -- so route/stop names decode correctly instead of the
/// ASCII-only, `?`-for-everything-else fallback `kr_roads::shapefile` uses
/// for MOCT's "display-only" Korean text (this CSV's names are join/lookup
/// keys, not just display, so a lossy decode isn't good enough here).
pub fn load_route_stops(csv_path: &Path) -> Result<RouteData, String> {
    let raw = std::fs::read(csv_path).map_err(|e| format!("{}: {e}", csv_path.display()))?;
    let (text, _, had_errors) = encoding_rs::EUC_KR.decode(&raw);
    if had_errors {
        return Err(format!("{}: CP949/EUC-KR decode had errors", csv_path.display()));
    }

    let mut stops_by_route: HashMap<String, Vec<RouteStop>> = HashMap::new();
    let mut malformed_rows = 0usize;
    for (line_no, line) in text.lines().enumerate() {
        if line_no == 0 || line.is_empty() {
            continue; // header
        }
        let mut cols = line.splitn(5, ',');
        let (Some(route_no), Some(seq_text), Some(stop_code), Some(stop_name)) =
            (cols.next(), cols.next(), cols.next(), cols.next())
        else {
            malformed_rows += 1;
            continue;
        };
        let Ok(seq) = seq_text.trim().parse::<u32>() else {
            malformed_rows += 1;
            continue;
        };
        stops_by_route.entry(route_no.trim().to_string()).or_default().push(RouteStop {
            route_no: route_no.trim().to_string(),
            seq,
            stop_code: stop_code.trim().to_string(),
            stop_name: stop_name.trim().to_string(),
        });
    }
    if malformed_rows > 0 {
        eprintln!("Warning: bus route CSV: {malformed_rows} row(s) skipped (missing/non-numeric column)");
    }
    for stops in stops_by_route.values_mut() {
        stops.sort_by_key(|s| s.seq);
    }

    Ok(RouteData { reference_date: REFERENCE_DATE, stops_by_route })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_first_four_columns_and_ignores_the_rest() {
        let dir = std::env::temp_dir();
        let path = dir.join("kr_bus_routes_test.csv");
        let (bytes, _, _) = encoding_rs::EUC_KR.encode(
            "노선번호,정류장순서,정류장코드,정류장명,선탑건수합계\n508,0,2600391,남부여객(종점),12345\n508,1,2600392,광명고교,999\n",
        );
        std::fs::write(&path, &bytes).unwrap();

        let data = load_route_stops(&path).unwrap();
        let stops = &data.stops_by_route["508"];
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0].seq, 0);
        assert_eq!(stops[0].stop_code, "2600391");
        assert_eq!(stops[0].stop_name, "남부여객(종점)");
        assert_eq!(stops[1].stop_name, "광명고교");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn sorts_stops_by_sequence_regardless_of_file_order() {
        let dir = std::env::temp_dir();
        let path = dir.join("kr_bus_routes_test_order.csv");
        let (bytes, _, _) = encoding_rs::EUC_KR.encode(
            "노선번호,정류장순서,정류장코드,정류장명,x\n10,2,c2,n2,0\n10,0,c0,n0,0\n10,1,c1,n1,0\n",
        );
        std::fs::write(&path, &bytes).unwrap();

        let data = load_route_stops(&path).unwrap();
        let stops = &data.stops_by_route["10"];
        assert_eq!(stops.iter().map(|s| s.seq).collect::<Vec<_>>(), vec![0, 1, 2]);

        std::fs::remove_file(&path).ok();
    }
}
