//! SPEC_Build.md M3 "교량과 완주" / SPEC_Bridge.md: 영도대교·부산대교의 상판
//! (아치·주탑·케이블·도개 기계실은 M5 -- 이 모듈은 SPEC_Bridge §3/§4가
//! 아니라 §6.1/§7만 구현한다: 상판, 접속부 전이).
//!
//! **교량 구간 판정: 좌표로 수동 지정.** SPEC_Bridge §9 자체가 "표준노드링크에
//! 교량 여부 필드가 있는지 확인 필요 -- 없으면 좌표로 수동 지정"이라고
//! 적어 뒀다. 실측 확인 결과: `ROAD_TYPE` 필드는 부산 전역에 2,185건이나
//! 걸려("003"이 교량 코드일 가능성을 먼저 의심했으나, 대상은 4건뿐이어야
//! 하므로 기각) 교량 여부와 무관한 값이다. `ROAD_NAME`에 "대교"가 든 링크도
//! 798건이 전국에 흩어져 있고 그나마도 실제로는 "대교로"(거리 이름)가
//! 대부분이라 교량 자체를 가리키지 않는다. 그래서 이 모듈은 버스
//! 정류소(실측 좌표 보유)를 기준점으로 삼아 표준노드링크에서 직접 후보를
//! 찾았다: "영도대교"/"영도대교.남포역"(남포동측), "대교사거리"(대교동측)
//! 정류소 좌표 인근에서 "대교로"라는 이름을 공유하는 4개 링크가 남포동
//! (1300001503/1300001502)에서 대교동(1330005802/1330005801)까지 하나로
//! 이어지는 것을 확인했다 -- 왕복 각 방향 2개 링크(중앙에서 갈라지는 게
//! 아니라 방향별로 완전히 분리된 선형)로, 이 모듈은 그 중심선(두 방향의
//! 평균)을 신설 교량 세그먼트의 경로로 쓴다.
//!
//! **부산대교는 이만큼 확정하지 못했다.** "중앙동"(중구측)·"부산대교입구"/
//! "봉래동교차로"(영도측) 정류소로 대략적인 진입부는 잡았지만, 표준노드링크
//! 안에서 그 사이를 잇는 특정 링크 열을 영도대교만큼 자신 있게 골라내지
//! 못했다 -- 그래서 `excluded_link_ids`가 비어 있다: 이 구간의 실제
//! 표준노드링크 링크가 지형 추종 도로로 별도로 놓일 수 있고, 그러면 상판과
//! 겹쳐 보일 수 있다. 실측 링크 열이 확인되면 채워 넣을 자리로 남겨 둔다.
//!
//! **남항대교·부산항대교는 아예 못 찾았다.** 508 및 대상 노선이 지나지
//! 않아(SPEC_GenerationScope §1.1) 정류소 앵커가 없고, A등급(자동차전용도로)
//! 링크를 영도 범위 안에서 검색해도 0건이었다 -- 이 데이터셋의 어떤
//! `ROAD_RANK`로 잡히는지 이 세션에서는 확인하지 못했다. 두 다리 모두
//! 비워 뒀다: 좌표를 지어내느니 빈 자리로 남기는 편이 낫다.

use super::RoadClass;

/// One manually specified bridge deck.
pub struct ManualBridge {
    pub name: &'static str,
    pub class: RoadClass,
    /// EN (EPSG:5186) waypoints from one shore to the other, in order.
    pub waypoints_en: &'static [(f64, f64)],
    /// Real 표준노드링크 node IDs at each end, when confirmed -- lets the
    /// deck's endpoint height come from the same node-height solve the rest
    /// of the network uses (§7: "교대 위치를 노드로 고정"), instead of an
    /// independent `Ground` sample that could disagree with it by a block
    /// or two. `None` falls back to sampling `Ground` directly at that
    /// waypoint.
    pub end_node_ids: [Option<&'static str>; 2],
    /// Real LINK_IDs this bridge's deck replaces -- excluded from the
    /// normal ground-following road pass so the crossing isn't drawn twice
    /// (once as a floating deck, once as a terrain-following road under/
    /// through it). Empty means no confirmed link to exclude yet (disclosed
    /// gap -- see the module doc).
    pub excluded_link_ids: &'static [&'static str],
}

/// 영도대교 (Yeongdo Bridge): 남포동(중구) <-> 대교동(영도구). Waypoints are
/// the centerline (average of the two real, direction-split MOCT carriageway
/// chains) -- see the module doc for how these were found and confirmed.
pub const YEONGDO_BRIDGE: ManualBridge = ManualBridge {
    name: "영도대교",
    class: RoadClass::B,
    waypoints_en: &[
        (385820.0, 280070.4),  // 남포동측 (mainland shore)
        (385912.7, 279676.65), // 중간 (mid-span)
        (386200.2, 279419.65), // 대교동측 (Yeongdo shore)
    ],
    end_node_ids: [Some("1300001503"), Some("1330005802")],
    excluded_link_ids: &["1330019200", "1330019201", "1330019100", "1330019102"],
};

/// 부산대교 (Busan Bridge): 중앙동(중구) <-> 봉래동(영도구). Waypoints are
/// bus-stop-anchored approximations (see module doc) -- less certain than
/// 영도대교's, and `excluded_link_ids` is empty because the real MOCT link
/// chain for this crossing wasn't confirmed this session.
pub const BUSAN_BRIDGE: ManualBridge = ManualBridge {
    name: "부산대교",
    class: RoadClass::C,
    waypoints_en: &[
        (385654.0, 280743.0), // 중앙동측 (mainland shore)
        (386306.0, 279398.0), // 봉래동측 (Yeongdo shore)
    ],
    end_node_ids: [None, None],
    excluded_link_ids: &[],
};

/// All bridges this M3 pass builds. 남항대교·부산항대교 are not here -- no
/// anchor could be confirmed this session (see the module doc); adding them
/// is a matter of appending another `ManualBridge`, not a structural change.
pub const MANUAL_BRIDGES: &[ManualBridge] = &[YEONGDO_BRIDGE, BUSAN_BRIDGE];
