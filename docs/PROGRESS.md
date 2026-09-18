# PROGRESS — 진행 상황

마지막 갱신: 2026-09-18. 이 문서 하나만 읽고 다음 세션을 이어갈 수 있게 쓴다.

---

## 0. 지금 당장 할 일

**2026-09-18 상태:**

1. uncommitted 횡단보도/정지선 코드 → 빌드 검증 → 커밋 `691c0596` → push.
2. 일반화 설계를 명세로 확정 (`SPEC_Scope_v0.2.md`, `SPEC_Validation_v0.1.md` 신규 + 7개 문서 수정) → 커밋 `77890c3c` → push.
3. **§7 item 1 완료 — `kr_transit`의 영도 하드코딩을 scope 판정으로 교체**, 커밋 `e456c168`, `cc296ba6` → push. 실측 검증 중 `SPEC_Scope §4.1.1` "조각의 두 역할"(route_strip은 자기 노선만 self-qualify)을 추가로 확정했다 — 자세한 내용은 이 절 바로 아래 대신 §7 item 1 본문 참고.
4. **`Scope::contains_en`을 L0-L2 물리 생성 범위에 연결했다** (item 1이 남겼던 것). `SPEC_Scope §2`:
   - L0 (지형 타일): `data_processing.rs`의 타일 필터링이 옛 route-polyline 버퍼 대신 `scope.contains_en(타일 중심, TERRAIN_BUFFER_M)`을 쓴다.
   - L1 (도로): `kr_roads::clip()`이 bbox 클립에 더해 `scope.contains_en(링크의 아무 점, STUB_LENGTH_M)` 필터를 추가로 건다 — 스텁은 **온전한 링크를 그대로 살리는 근사**다(정확한 중간 절단+지형 테이퍼는 안 함, `SPEC_RoadProfile P8` 미연결 상태 그대로).
   - L2/L3 (건물): `kr_buildings::compute_kr_buildings`가 옛 `BUILDING_BUFFER_M`+`route_polylines` 근접 거리 판정 대신 `scope.contains_en(중심점, 0.0)`을 직접 쓴다. **동작이 바뀐 지점**: scope 밖 + 저층인 건물은 이제 **아예 생략**된다(전엔 전부 생성했다) — `KrBuildingsReport.omitted_out_of_scope`로 집계.
   - `data_processing.rs`의 M2/M1/M4 순서를 재배치했다 — `kr_transit::build_m2`(scope 산출)가 M1(`kr_roads`)보다 먼저 실행되어야 L1이 그 `Scope`를 받을 수 있다. 의존 방향 확인됨 (M2는 M1의 `KrRoadNetwork`를 쓰지 않는다, 자기만의 라우팅 그래프를 따로 읽는다).
   - `kr_roads::block_to_en` (`en_to_block`의 역함수), `KoreaTmProjection::unproject_raw`, `Scope::contains_en` 추가.
   - **검증됨 (compute 단계만)**: 단위 테스트로 `kr_scope`(7개, EN 왕복 포함)와 `kr_roads::clip()`(scope 포함/제외 양쪽, 합성 fixture) 직접 검증 + `kr_transit`의 실데이터 M2 테스트가 여전히 baseline과 byte-identical. 커밋 `d291c1b1` → push.
   - **검증 안 됨 (미룬 것이지 면제된 것이 아니다)**: 실제 `arnis.exe`를 돌려 Anvil 월드를 생성하고, 사각형만 vs 사각형+508 띠 두 실행의 결과를 블록 단위로 비교하는 것 — 사용자가 원래 요청한 "실행" 검증 — 은 **아직 하지 않았다.** 위 compute-단계 검증은 그 대체물이 아니다: `Scope::contains_en`이 옳은 좌표를 옳게 판정한다는 것과, 그 판정이 실제 생성 파이프라인 전체(지형 보간·도로 종단선형·건물 파사드 등)를 거쳐 baseline과 같은 블록을 낳는다는 것은 별개의 주장이다. §6 "scope 연결 실행 검증" 항목으로 남겨뒀다 — SCALE 파라미터화 작업(§7 item 5)의 전체 실행에 얹어서 한 번에 한다.

**다음 세션(또는 이 세션 재개)이 할 일** — §7의 나머지: 2번(교량 프리셋 파일 읽기), 5번(`SCALE` 파라미터화 — 끝나면 그 실행에 §6 "scope 연결 실행 검증"을 얹을 것).

**빌드 전 메모리 확인은 여전히 유효하다.**

```powershell
Get-CimInstance Win32_OperatingSystem | Select-Object FreePhysicalMemory,TotalVisibleMemorySize
```
빌드 절차는 §5 "Windows 빌드 유의사항" 참고.

---

## 1. 이 프로젝트가 뭔가

McARN(`AI_Projects/McARN/`, 패키지명 `arnis`, 바이너리 `arnis.exe`)은 오픈소스 Arnis(github.com/louis-e/arnis) 포크다. `--input-source kr` 플래그로 한국 공공 GIS 데이터(표준노드링크 도로망, 건물통합정보, 부산 버스 정류소/노선)를 읽어 실제 지형·도로·건물을 그대로 재현한 Minecraft 월드를 생성한다. 1차 대상 지역은 영도구(부산).

스펙 문서는 `docs/SPEC_*.md`에 있다 (Build/Ingest/RoadProfile/RoadSection/BuildingType/Bridge/StreetFurniture는 `_v0.1`, 범위 정의는 `SPEC_Scope_v0.2.md` — 구 `SPEC_GenerationScope_v0.1.md`는 폐기됨 — 검증 지표는 `SPEC_Validation_v0.1.md`, 신규). `SPEC_Build_v0.1.md`가 마일스톤 정의: M0(지형) → M1(도로) → M2(버스, 선택) → M3(교량) → M4(건물) → M5(가로 요소). M0~M5는 구현 순서이고, 한 번 실행할 때 무엇을 생성할지는 별도 `--quality` 옵션이 정한다 (`SPEC_Build §2`).

McBPT/McBLS/McRIG과는 무관한 별개 프로젝트 — 이쪽은 독립 월드 생성기, 저쪽은 Minecraft 애드온/플러그인.

---

## 2. M0~M4 — 완료

| 마일스톤 | 내용 | 커밋 |
|---|---|---|
| M0 | 지형 (`--input-source kr` 좌표 변환 + 표고) | `6eb6207e` |
| M1 | 표준노드링크 도로 ingestion + 배치 | `ca175d23`, `dbc8d97c` |
| M2 | 버스 정류소/노선 매칭, L0 버퍼 타일 스코핑 | `98197728` |
| M3 | 영도대교·부산대교 수동 교량 | `7a14e938` |
| M4 | 건물통합정보 ingestion, 유형/파사드/경사 | `e26d394c` |

P1 스코프(영도 전역, `35.046,129.028,35.100,129.092`)로 전체 생성 1회 완주 확인함 (2026-09-14): 건물 14,843개 로드, 14,270개 배치.

---

## 3. M5 "가로 요소" — 진행 중

SPEC_StreetFurniture.md §0 우선순위 그대로: 전주·전선 → 버스정류장 → 가로등 → 나머지. 새 모듈 `src/kr_street_furniture/mod.rs`.

| 항목 | 상태 | 커밋 |
|---|---|---|
| §2 전주·전선·인입선 | **완료, 검증됨, 푸시됨** | `f03c50e9` |
| §3 버스정류장 (승차대·표지·노면 표시) | **완료, 검증됨, 푸시됨** | `f1e182c4` |
| SPEC_RoadSection §3 중앙선(황색 실선) | M1부터 이미 동작 (`Band::MedianPaint`) | — |
| SPEC_RoadSection §3 횡단보도·정지선 | **코드 작성됨, 빌드 미검증, uncommitted** | — |
| SPEC_RoadSection §3 차선 점선 (3차로 이상) | **의도적으로 미착수** — 아래 참고 | — |
| RED_CONCRETE 임시 하이라이트 원복 | **코드 작성됨, 빌드 미검증, uncommitted** | — |
| §4 가로등 | **미착수** | — |
| §6 교통시설(신호등·도로표지·볼라드), §7 기타(소화전·우편함·쓰레기통·맨홀), §5 가로수 | **미착수** | — |

**차선 점선을 미룬 이유**: `Segment.lanes`에 실제 값(1/2/4/6, 실측 확인됨)이 있지만, 현재 단면 모델(`section_spec`)은 등급별로 `carriage_each`(차도 반폭)가 고정폭 하나의 밴드다 — 실제 차로 수만큼 그 밴드를 세분하는 설계를 안 했다. 손대려면 `cross_section_layout`을 확장해야 한다.

**§8 배치 우선순위(버스정류장 > 신호등 > 전주 > 도로표지 > 가로등 > 가로수 > 볼라드·기타)**는 `kr_street_furniture::ClaimedColumns`로 구현 중 — 지금 존재하는 두 요소(버스정류장, 전주)는 정류장이 먼저 클레임하고 전주가 그 뒤를 체크하는 순서로 맞춰뒀다. 새 요소를 추가할 때마다 이 순서를 지켜서 호출 위치를 넣을 것 (`data_processing.rs`에서 병렬/순차 경로 둘 다).

---

## 4. 해결된 주요 이슈 (다음에 비슷한 증상 보면 여기부터 봐라)

1. **도로 배치 ~20% 조용히 실패** — `kr_roads`가 지형 생성에 자기 목표 높이를 안 알려줘서(`highways.rs`는 `register_road_surface_y`로 알려주는데 `kr_roads`는 안 함), 지형이 도로 자리를 실제 DEM 높이로 먼저 채우고, 도로 쓰기는 `None,None`(비어있을 때만 쓰기)라 조용히 졌다. **고침**: `register_ground_overrides`(지형 생성 전에 높이 선등록) + 쓰기를 빈 블랙리스트로(강제 덮어쓰기). 하나만으론 안 됨 — 등록만 하면 지형이 도로 높이에 정확히 맞춰 채워져서 여전히 짐. 둘 다 필요. (`bcdb7f6b`, `b0da886f`)

2. **경사 지형에서 "절벽" 현상** — 도로 자체 높이(Hc)와 원지형(H0, `terrain_level`)을 도로 위 지점에서 직접 비교하니 거의 일치(중앙값 0, 최대 3, 52,961개 샘플 중 임계치 초과 0건) — height-solve는 정확했다. 문제는 도로 가장자리 바로 밖 지형이 급경사인데 그걸 완충할 게 없었던 것(SPEC_RoadProfile P8 미구현, 이미 disclosed였음). **고침**: P8 구현 — 낙차 작으면 4블록 테이퍼(`register_taper`), 크면(`RETAINING_THRESHOLD=3` 초과) 옹벽(`place_retaining_walls`, STONE_BRICKS). (`f044f897`)

3. **`furniture_column`이 좌측 전주를 0개 배치** — 인도-연석 전이를 찾는 로직이 "연석 다음에 오는 인도"만 잡아서, 배열 순서상 연석이 인도 *뒤에* 오는 좌측은 절대 안 잡혔다. 연석 셀의 양쪽 이웃을 다 확인하도록 고침.

4. **Windows 빌드가 `aws-lc-sys`에서 실패** — repo 경로에 한글(`바탕 화면`)이 있어서 NASM/MSVC 빌드 스크립트가 깨짐. `--target-dir`를 ASCII 경로(`/c/mcarn_build`)로 돌리면 해결. NASM 자체는 PATH에 없고 winget 설치 경로에 있음 (§5 참고).

5. **프리뷰 렌더러가 도로와 건물을 색으로 구분 못 함** — `kr_buildings`와 `kr_roads`가 같은 블록 팔레트(LIGHT_GRAY_CONCRETE 등)를 써서 상면 렌더에서 안 갈렸다. 임시로 RED_CONCRETE로 도로를 칠해서 확인했고, §3(도색)이 들어가면서 실제 팔레트로 원복하는 중(§0 참고, uncommitted).

6. **버퍼 스코핑 경계 아티팩트** — M2 buffer scoping에서 제외된 타일에 걸친 세그먼트는 그 구간에 도로도 지형도 없다(`-56` 높이의 이상하게 평평한 `grass_block` 등) — 버그 아님, 스코프 밖이라 아예 생성 안 된 것. 다음에 높이/지형 측정할 때 스코프 경계 근처면 이 가능성부터 배제할 것.

---

## 5. Windows 빌드 유의사항

```bash
# NASM을 PATH에 추가 (winget으로 설치돼 있지만 PATH엔 없음)
export PATH="/c/Users/82108/AppData/Local/Microsoft/WinGet/Packages/BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe/mingw64/bin:$PATH"

cd "/c/Users/82108/OneDrive/바탕 화면/AI_Projects/McARN"
cargo build --release --no-default-features --target-dir /c/mcarn_build
```

- `--target-dir`을 ASCII 경로로 돌리는 게 필수 — repo 경로의 한글이 `aws-lc-sys` 빌드를 깬다.
- `--no-default-features`로 Tauri GUI를 꺼야 NASM 관련 다른 실패를 피한다.
- 빌드된 바이너리: `/c/mcarn_build/release/arnis.exe`.
- 메모리 부족으로 죽으면: 다른 프로그램 정리 → 여유 메모리 확인 → 그래도 안 되면 `-j 1`, `CARGO_PROFILE_RELEASE_LTO=false` 시도해볼 수는 있으나 지난 세션엔 그것도 안 먹혔다 — 근본적으로 여유 메모리 자체가 부족했던 것으로 보임.

실행 예시 (P0, 영선동 1.5km):
```bash
arnis.exe --input-source kr \
  --bbox "35.0695,129.0320,35.0829,129.0484" \
  --kr-roads-dir "data/[2026-08-12]NODELINKDATA" \
  --kr-bus-stops-dir "data/부산광역시_버스 정류소 정보(SHP)_20250121" \
  --kr-bus-routes-csv "data/부산광역시_버스노선별 승하차 정보_20230731.csv" \
  --kr-buildings-shp "data/AL_D010_26_20260909/AL_D010_26_20260909.shp" \
  --terrain-cache-dir <ascii 경로> \
  --output-dir <ascii 경로> \
  --map-preview --downloader curl
```

월드 검증은 Rust 안 거치고 Node(`prismarine-provider-anvil`/`prismarine-chunk`)로 저장된 Anvil 리전을 직접 읽는 스크립트를 매번 그때그때 짜서 했다. **2026-09-18부터는 아니다** — `scripts/anvil-diff/`에 재사용 가능한 버전을 남겼다(두 월드 폴더 + bbox를 받아 청크 단위 블록 diff 요약을 낸다; 합성 월드를 직접 만들어 실행하는 자체 self-test 포함, `npm test`). 다음에 Anvil 단계 검증이 필요하면 이걸 쓴다 — 새로 짜지 말 것. 사용법은 그 폴더의 `README.md`.

---

## 6. 남은 과제

**M5 마저 구현** (§0 우선순위 순서):
1. §2/§3 커밋된 것 위에 uncommitted 횡단보도/정지선/팔레트 원복 — 빌드 확인 후 커밋 (§0 참고)
2. §4 가로등 (양쪽 교대 배치, 48블록 간격) — 미착수
3. 차선 점선 (3차로 이상) — `cross_section_layout` 확장 필요, 위 §3 참고
4. §6 교통시설(신호등·도로표지·볼라드), §7 기타(소화전·우편함·쓰레기통·맨홀), §5 가로수 — 전부 미착수. §8 우선순위표 전체(신호등이 전주보다 높음, 도로표지가 가로등보다 높음 등) 아직 부분적으로만 구현됐다는 뜻 — 나머지 요소 추가할 때 순서 다시 챙길 것.

**P1(영도 전역) 재생성** — M5 요소들이 들어간 채로 전체 스코프 한 번 다시 돌려서 완주 확인 안 함 (지금까지는 대교동/남항동 소구역 + P0 1.5km만 테스트).

**프리뷰 렌더러 자체 개선** — 지금은 도로 색을 실제 팔레트로 되돌리는 것으로 건물과의 충돌을 해결했지만(§4의 SPEC_RoadSection §3 도색이 실질적 구분 신호), `map_renderer.rs`가 `road_surface_overrides`를 참고해서 블록 색이 아니라 "이게 도로다"라는 사실 자체로 구분하게 만드는 게 근본적 수정이다. 지금은 안 건드림.

**scope 연결 실행 검증** (2026-09-18, §7 item 1의 마지막 조각 — 미룬 것이지 면제된 것이 아니다). `Scope::contains_en`을 L0/L1/L2 생성 범위에 연결한 코드(커밋 `d291c1b1`)는 단위 테스트와 실데이터 M2 baseline 비교로만 검증됐다 — 실제 바이너리로 월드를 생성해 블록 단위로 비교하는 검증은 아직 하지 않았다. **별도로 돌리지 말고 §7 item 5(`SCALE` 파라미터화) 작업의 전체 실행에 얹는다** (그 작업도 어차피 실제 실행 검증이 필요하므로 한 번에 한다). 절차:

1. 영도 사각형만(scope 조각에서 508 route_strip 제외) 1회 실행 → 월드 A
2. 영도 사각형 ∪ 508 노선 띠(현재 프리셋 그대로) 1회 실행 → 월드 B
3. `scripts/anvil-diff`로 A/B를 **사각형 bbox 안에서만** 비교 (`node anvil-diff.js A B --bbox <사각형의 블록 좌표>`) → `totalDiffBlocks: 0`이어야 한다 (사각형 구간은 baseline과 일치, 즉 508 띠 추가가 기존 결과를 건드리지 않는다는 뜻)
4. 월드 B의 `region/` 폴더에 508 띠 구간(남포동~부산역 쪽) 청크가 실제로 존재하는지 확인 (508 띠 구간은 추가 생성됐다는 뜻 — `anvil-diff`는 두 월드가 공유하는 bbox 안의 "일치 여부"만 보므로 이 부분은 별도 확인)
5. 3번에서 손실(diff)이 나오면 멈추고 원인부터 보고 — 진행하지 않는다

---

## 7. 일반화 계획 — 영도 말고 다른 지역에도 쓰려면

지금 코드는 여전히 영도구 하나만 놓고 만들어져 있다 — **이 절이 가리키는 코드 전수 조사는 2026-09-16 것 그대로다. 2026-09-18에 바뀐 건 설계(명세)뿐, 코드는 아직 손대지 않았다.** 아래 각 항목에 그 사이 확정된 명세 위치를 달아뒀다. 다음 세션은 이 명세를 따라 코드를 고치면 된다.

### 구조적으로 막힌 곳 (코드 수정 필요)

1. **완료 (2026-09-18, 커밋 `e456c168`).** `src/kr_transit/mod.rs`의 `PRIMARY_ROUTE`/`YEONGDO_LON_MIN/MAX`/`YEONGDO_LAT_MIN/MAX`/`is_in_yeongdo_range`를 전부 제거하고, 새 `src/kr_scope/mod.rs`(`Scope`/`ScopePiece`)의 scope 판정으로 대체했다. `kr_transit::resolve_scope_pieces`가 프리셋의 `ScopePieceInput`(`kr_scope::presets::yeongdo()`)을 실제 지오메트리로 바꾸고, 노선 채택·정류소 절단 둘 다 `Scope::contains_for_route` 하나로 판정한다.
   - **구현 중 명세가 한 번 더 갈렸다.** route_strip 조각을 모든 노선에 똑같이 적용되는 전역 판정(`Scope::contains`)에 썼더니, 508이 지나는 도심 환승 거점(남포동·중앙동·초량·부산역)을 스치기만 하는 무관한 노선까지 채택돼 노선 수가 20→51개로 늘었다(실측). `SPEC_Scope_v0.2.md §4.1.1` "조각의 두 역할"로 해소 — 영역 조각(rect/admin_polygon)은 모든 노선의 채택 판정에 쓰이고, 노선 조각(route_strip)은 **자기 노선의 판정에만** 관여한다. 지형·도로·건물 생성 범위(아직 코드에 안 붙어 있음)는 여전히 전 조각의 순수 합집합(`Scope::contains`)을 쓴다 — 예외는 노선/정류소 채택뿐이다.
   - **검증**: 영도 사각형 ∪ 508 route_strip 프리셋으로 실데이터(`data/`)를 돌려, 리팩터 전 스냅샷(`stops_review_BASELINE_pre_scope_refactor.json`)과 정규화 비교 — 완전 일치(20개 노선, 161개 정류소, route별 kept/cut까지 전부 동일).
   - 남은 일: `Scope::contains`(전역 물리 scope)를 실제 L0/L1/L2 지형·도로·건물 생성 범위에 연결하는 건 아직 안 했다 — 지금은 `kr_transit`의 노선/정류소 판정에만 쓰인다.

2. **`src/kr_roads/bridges.rs`의 `MANUAL_BRIDGES`가 영도대교·부산대교 두 개로 완전히 하드코딩돼 있다.**
   - 표준노드링크 데이터 자체에 교량 여부 필드가 없어서, 이 두 다리는 좌표(EPSG:5186 waypoints)와 MOCT 노드 ID를 손으로 찾아 박아넣은 것이다(M3 module doc에 이미 명시).
   - **스키마가 확정됐다.** `SPEC_Scope_v0.2.md §5.1`의 `[[bridges]]`가 그 설정 파일 형식이다 — waypoints, MOCT 노드 ID, 제외 링크 ID, 형식, 제원. `SPEC_Bridge_v0.1.md §0`도 "교량 목록은 프리셋 파일에서 온다"고 명시하도록 고쳤다. 하드코딩된 `&[ManualBridge]` 상수를 이 프리셋 필드를 읽는 코드로 바꾸면 된다. 여전히 사람이 표준노드링크를 뒤져서 교량 좌표를 찾아야 하는 것 자체는 못 피한다 — 그건 구조가 아니라 데이터 확보의 문제다.

### 확인은 필요하지만 구조는 괜찮은 곳

3. **Korea TM 투영(`src/projection/korea_tm.rs`)은 실제로 이미 일반적이다** — 원점(E0/N0)을 실행마다 `--bbox`/`--bbox-en`에서 계산한다, 하드코딩된 지역 좌표 없음. **서부/동부 TM 원점 자동 선택은 하지 않기로 결정했다** — `SPEC_Ingest_v0.1.md §2.1`에 이유를 명시했다: 전국 단일 좌표 프레임이라 인접 지역 맵이 이어 붙고, 국내 최원거리에서도 축척 오차 0.1% 미만이라 실용적 문제가 없다. EPSG:5186 하나로 고정하고, 다른 원점이 필요하면 프리셋 `overrides`로 덮어쓰는 것만 허용한다. 이 항목은 더 이상 "확인 필요"가 아니라 **결정 완료**다.
4. **`scale = 1.75`가 `--input-source kr`에서 고정값으로 강제된다.** 이제는 아니다 — `SPEC_Ingest_v0.1.md §2.2`가 `SCALE`을 실행 파라미터(기본값 1.75)로 바꿨다. 코드에서 상수를 파라미터로 빼는 작업이 남았다. 연동 범위가 크다 — `SPEC_RoadSection`/`SPEC_StreetFurniture`/`SPEC_BuildingType`/`SPEC_Bridge`의 모든 치수 표가 "실제값 × `SCALE`" 공식으로 다시 쓰였으므로, `SCALE`을 파라미터화하는 코드 변경은 이 문서들이 정의한 공식을 그대로 구현하는 작업이 된다.
5. **`kr_buildings`의 건물통합정보 `.dbf` 컬럼 매핑(`A9`=주용도, `A13`=사용승인일 등)이 영도 실 데이터를 샘플링해서 역추적한 것이다** (컬럼명이 전부 익명화된 배포본이라 공식 필드 사전이 없다). **이제 이 매핑 자체를 설정으로 분리하는 구조가 정해졌다** — `SPEC_Scope §5.1`의 `buildings.dbf_schema`가 이름으로 가리키는 매핑 파일이다. 추가로 `SPEC_Ingest §4.1`이 적재 시 자기 검증(날짜 형식 확인, 용도 코드집합 대조, 실패 시 중단)을 필수로 요구하도록 바뀌었다 — 이게 있으면 다른 지역에서 매핑이 어긋나도 결측값으로 조용히 새는 대신 그 자리에서 멈춘다. 여전히 **데이터 검증 리스크**는 남는다 — 다른 지역 `.dbf`가 같은 스키마를 쓰는지는 실제로 넣어봐야 안다.
6. **`kr_bus_routes::REFERENCE_DATE = "2023-07-31"`** — 지금 쓰는 CSV 파일 자체의 기준일. 다른 CSV(다른 지역이든 최신판이든)를 쓸 때 이 상수가 실제로 그 파일에서 읽어오는 게 아니라 고정값이면, 파일을 바꿔도 `manifest.json`엔 옛날 날짜가 찍힌다. `SPEC_Scope §5.1`의 `transit.baseline_date`가 이 값을 프리셋에서 지정하는 자리를 정의했지만, "CSV에서 직접 읽어올지" 여부는 아직 코드 결정으로 남아 있다.
7. **CLI 플래그 자체(`--kr-bus-stops-dir` 등)는 이미 일반적**이다(임의 경로를 받음) — `args.rs`의 doc comment가 "부산 버스 정류소 SHP"라고 못박아 써놔서 다른 지역 데이터를 넣어도 되는지 헷갈릴 수 있다는 것뿐. 코드 문제 아니고 문서 문구 문제.

### 하지 않아도 되는 것

- `kr_roads`의 도로망 클리핑(`clip()`)은 이미 실행마다 넘어온 bbox로 동작한다 — 하드코딩 없음.
- `kr_street_furniture`의 모든 상수(간격·크기 등)는 지역 무관 설계 수치다 — `SPEC_StreetFurniture.md` 자체가 전국 공통으로 잡은 값들이고, 이제 축척 공식으로 재정의됐다(§4).
- `MOCT_LINK.dbf`/`MOCT_NODE.dbf` 파일명은 표준노드링크 자체의 전국 공통 스키마 — 손댈 필요 없음.

### 우선순위 제안

**1번 완료 (2026-09-18).** 다음은 4번(`SCALE` 파라미터화) — 1번과 맞물려 있어서(둘 다 `kr_transit`/스케일 관련 상수를 실행 파라미터로 빼는 작업) 이어서 하는 게 효율적이다. 2번(교량)은 프리셋에 없으면 다리 없이 돌아가도록 설계됐으니(`SPEC_Bridge §0`) 급하지 않다 — §1의 완주 조건에서 "교량 연결"도 이미 뺐다(`SPEC_Build §1`). 3, 5~7번은 실제로 다른 지역 데이터를 넣어보면서 하나씩 걸리는 대로 고치면 된다.
