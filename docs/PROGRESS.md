# PROGRESS — 진행 상황

마지막 갱신: 2026-09-16. 이 문서 하나만 읽고 다음 세션을 이어갈 수 있게 쓴다.

---

## 0. 지금 당장 할 일

**빌드부터 확인하라.** 지난 세션은 `cargo build`가 시스템 메모리 부족으로 4번 연속(기본, `-j 2`, `-j 1`, `CARGO_PROFILE_RELEASE_LTO=false` 전부 시도) 죽어서 코드를 검증도 못 하고 끝났다.

```powershell
Get-CimInstance Win32_OperatingSystem | Select-Object FreePhysicalMemory,TotalVisibleMemorySize
```
여유 메모리가 몇 GB는 되는지 먼저 확인. 그 다음 빌드 절차는 §5 "Windows 빌드 유의사항" 참고.

**working tree에 uncommitted 변경이 남아있다** (`git status`로 확인):
- `src/kr_roads/mod.rs` — `DEBUG_ROAD_HIGHLIGHT`(RED_CONCRETE) 원복 + `carriageway_half_width`/`endpoints` accessor 추가
- `src/kr_street_furniture/mod.rs` — `place_crosswalks` (횡단보도·정지선)
- `src/data_processing.rs` — 위 둘 연결

**이 코드는 한 번도 컴파일 성공한 적이 없다.** 빌드되면 그 다음에: 대교동/남항동 확대 렌더 + `road-continuity check`가 여전히 0인지 확인한 뒤 커밋. 안 되면 에러 고치고 다시.

---

## 1. 이 프로젝트가 뭔가

McARN(`AI_Projects/McARN/`, 패키지명 `arnis`, 바이너리 `arnis.exe`)은 오픈소스 Arnis(github.com/louis-e/arnis) 포크다. `--input-source kr` 플래그로 한국 공공 GIS 데이터(표준노드링크 도로망, 건물통합정보, 부산 버스 정류소/노선)를 읽어 실제 지형·도로·건물을 그대로 재현한 Minecraft 월드를 생성한다. 1차 대상 지역은 영도구(부산).

스펙 문서는 전부 `docs/SPEC_*_v0.1.md`에 있다 (Build/Ingest/GenerationScope/RoadProfile/RoadSection/BuildingType/Bridge/StreetFurniture). `SPEC_Build_v0.1.md`가 마일스톤 정의: M0(지형) → M1(도로) → M2(버스) → M3(교량) → M4(건물) → M5(가로 요소).

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

월드 검증은 Rust 안 거치고 Node(`prismarine-provider-anvil`/`prismarine-chunk`)로 저장된 Anvil 리전을 직접 읽는 스크립트를 그때그때 짜서 했다 — 재사용 가능한 스크립트로 남겨두진 않았음. 다음에 비슷한 검증이 필요하면 이 패턴(region 파일 목록 → `anvil.load(cx,cz)` → `chunk.getBlock`)을 다시 짜면 된다.

---

## 6. 남은 과제

**M5 마저 구현** (§0 우선순위 순서):
1. §2/§3 커밋된 것 위에 uncommitted 횡단보도/정지선/팔레트 원복 — 빌드 확인 후 커밋 (§0 참고)
2. §4 가로등 (양쪽 교대 배치, 48블록 간격) — 미착수
3. 차선 점선 (3차로 이상) — `cross_section_layout` 확장 필요, 위 §3 참고
4. §6 교통시설(신호등·도로표지·볼라드), §7 기타(소화전·우편함·쓰레기통·맨홀), §5 가로수 — 전부 미착수. §8 우선순위표 전체(신호등이 전주보다 높음, 도로표지가 가로등보다 높음 등) 아직 부분적으로만 구현됐다는 뜻 — 나머지 요소 추가할 때 순서 다시 챙길 것.

**P1(영도 전역) 재생성** — M5 요소들이 들어간 채로 전체 스코프 한 번 다시 돌려서 완주 확인 안 함 (지금까지는 대교동/남항동 소구역 + P0 1.5km만 테스트).

**프리뷰 렌더러 자체 개선** — 지금은 도로 색을 실제 팔레트로 되돌리는 것으로 건물과의 충돌을 해결했지만(§4의 SPEC_RoadSection §3 도색이 실질적 구분 신호), `map_renderer.rs`가 `road_surface_overrides`를 참고해서 블록 색이 아니라 "이게 도로다"라는 사실 자체로 구분하게 만드는 게 근본적 수정이다. 지금은 안 건드림.

---

## 7. 일반화 계획 — 영도 말고 다른 지역에도 쓰려면

지금 파이프라인은 영도구 하나만 놓고 만들어졌다. 다른 지역(예: 부산 다른 구, 다른 도시)에 쓰려면 구조적으로 막혀있는 지점이 두 곳, 데이터 포맷 리스크가 한 곳 있다. 코드 전수 조사 결과(2026-09-16):

### 구조적으로 막힌 곳 (코드 수정 필요)

1. **`src/kr_transit/mod.rs`의 "이 지역인가?" 판정 전체가 영도 전용이다.**
   - `PRIMARY_ROUTE = "508"` (line ~35) — 항상 전량 포함시키는 "주 노선"이 하드코딩. 다른 지역이면 다른 노선번호이거나 아예 "주 노선" 개념이 없을 수 있다.
   - `YEONGDO_LON_MIN/MAX`, `YEONGDO_LAT_MIN/MAX` (line ~37-40) + `is_in_yeongdo_range()` — 행정경계 폴리곤이 없어서 위경도 사각형으로 "영도구인가"를 대신 판정한다. 이게 노선/정류소가 "이 실행 범위에 속하는가"를 가르는 유일한 신호다.
   - **일반화 방향**: 이 둘을 CLI 플래그나 별도 설정 파일로 빼야 한다 — 예: `--kr-primary-route <노선번호>` (선택), `--kr-scope-bbox <lat,lon,lat,lon>` 또는 실제 행정경계 폴리곤 파일 경로. 폴리곤을 쓸 수 있으면 사각형보다 정확해지고, 지역 경계 근처 정류소 오분류(§1.1에 이미 disclosed된 한계) 문제도 줄어든다.

2. **`src/kr_roads/bridges.rs`의 `MANUAL_BRIDGES`가 영도대교·부산대교 두 개로 완전히 하드코딩돼 있다.**
   - 표준노드링크 데이터 자체에 교량 여부 필드가 없어서, 이 두 다리는 좌표(EPSG:5186 waypoints)와 MOCT 노드 ID를 손으로 찾아 박아넣은 것이다(M3 module doc에 이미 명시).
   - **일반화 방향**: 이건 "코드를 고친다"로 해결 안 된다 — 다른 지역 쓰려면 그 지역 교량을 똑같이 수작업으로 찾아서 항목을 추가해야 한다. 할 수 있는 일반화는: 하드코딩된 `&[ManualBridge]` 상수 대신 **지역별 교량 설정 파일**(JSON/TOML, waypoints+node ID+제외 링크 목록)을 읽게 바꿔서, 다음 지역을 추가할 때 코드를 다시 컴파일하지 않고 파일만 추가하면 되게 만드는 것. 여전히 사람이 표준노드링크를 뒤져서 교량 좌표를 찾아야 하는 건 못 피한다.

### 확인은 필요하지만 구조는 괜찮은 곳

3. **Korea TM 투영(`src/projection/korea_tm.rs`)은 실제로 이미 일반적이다** — 원점(E0/N0)을 실행마다 `--bbox`/`--bbox-en`에서 계산한다, 하드코딩된 지역 좌표 없음. 다만 항상 EPSG:5186(중부원점, 127°E)을 쓴다 — 한국은 서부/중부/동부 3개 TM 원점이 따로 있는데, 그중 중부만 쓰는 게 고정이다. 영도(129°E)는 중부원점에서 좀 떨어져 있어도 수치적으로는 문제없이 동작하지만, 다른 지역이 서부/동부원점 관할이면 "공식적으로 맞는" 원점은 아니게 된다. 엄밀히 하려면 bbox 경도로 원점을 자동 선택하는 로직이 필요할 수 있음 — 급한 건 아니다.
4. **`scale = 1.75`가 `--input-source kr`에서 고정값으로 강제된다** (SPEC_Ingest §2 근거). 지역 문제는 아니지만, 다른 축척을 쓰고 싶은 경우를 위한 조정 여지는 없다.
5. **`kr_buildings`의 건물통합정보 `.dbf` 컬럼 매핑(`A9`=주용도, `A13`=사용승인일 등)이 영도 실 데이터를 샘플링해서 역추적한 것이다** (컬럼명이 전부 익명화된 배포본이라 공식 필드 사전이 없다). 다른 지역 `.dbf`가 같은 스키마를 쓰는지 확인 안 됨 — 다른 지역 데이터를 처음 넣을 때 이 매핑이 맞는지부터 검증해야 한다. 코드 구조 문제가 아니라 **데이터 검증 리스크**.
6. **`kr_bus_routes::REFERENCE_DATE = "2023-07-31"`** — 지금 쓰는 CSV 파일 자체의 기준일. 다른 CSV(다른 지역이든 최신판이든)를 쓸 때 이 상수가 실제로 그 파일에서 읽어오는 게 아니라 고정값이면, 파일을 바꿔도 `manifest.json`엔 옛날 날짜가 찍힌다 — 다음에 손댈 때 CSV에서 직접 읽어오게 바꿀지 확인할 것.
7. **CLI 플래그 자체(`--kr-bus-stops-dir` 등)는 이미 일반적**이다(임의 경로를 받음) — `args.rs`의 doc comment가 "부산 버스 정류소 SHP"라고 못박아 써놔서 다른 지역 데이터를 넣어도 되는지 헷갈릴 수 있다는 것뿐. 코드 문제 아니고 문서 문구 문제.

### 하지 않아도 되는 것

- `kr_roads`의 도로망 클리핑(`clip()`)은 이미 실행마다 넘어온 bbox로 동작한다 — 하드코딩 없음.
- `kr_street_furniture`의 모든 상수(간격·크기 등)는 지역 무관 설계 수치다 — SPEC_StreetFurniture.md 자체가 전국 공통으로 잡은 값들.
- `MOCT_LINK.dbf`/`MOCT_NODE.dbf` 파일명은 표준노드링크 자체의 전국 공통 스키마 — 손댈 필요 없음.

### 우선순위 제안

다른 지역을 실제로 넣어보게 된다면 위 1번(`kr_transit` 스코프 판정)부터 손대는 게 맞다 — 이게 없으면 그 지역 버스 노선이 아예 하나도 안 걸린다. 2번(교량)은 데이터가 없으면 그냥 다리 없이 돌아가니(§1의 완주 조건은 통과 못하겠지만) 급하지 않다. 3~7번은 실제로 다른 지역 데이터를 넣어보면서 하나씩 걸리는 대로 고치면 된다.
