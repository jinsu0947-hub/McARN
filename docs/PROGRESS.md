# PROGRESS — 진행 상황

마지막 갱신: 2026-09-19. 이 문서 하나만 읽고 다음 세션을 이어갈 수 있게 쓴다.

---

## 0. 지금 당장 할 일

**2026-09-19 상태 (2차) — SCALE 파라미터화 완료 + scope/bbox 구조적 버그 발견·수정:**

`PROGRESS.md §7` item 4(`SCALE` 파라미터화)를 완료했다. 작업 도중 scope와
`--bbox`의 관계에 구조적 결함이 있는 것을 발견해(§6 "scope 연결 실행 검증"과
같은 뿌리) 함께 고쳤다. 커밋 전 상태 — 다음 세션은 검증 결과를 확인하고
커밋할 것.

**1) SCALE 파라미터화 (`SPEC_Ingest §2.2`, `PROGRESS.md §7` item 4)**

- `args.rs`: `--input-source kr`가 더 이상 `scale`을 1.75로 무조건 덮어쓰지
  않는다. `main.rs`가 `Args::parse()` 대신 `clap::ArgMatches`를 직접 써서
  "사용자가 `--scale`을 실제로 타이핑했는가"(`scale_explicit`)를
  `apply_input_source_defaults`에 넘긴다 — clap의 `default_value_t`만으로는
  "안 씀"과 "명시적으로 1.0을 씀"을 구분할 수 없어서(둘 다 `f64` 값 1.0으로
  같다) 필요했던 우회. 기본값 1.75는 유지, 명시하면 그 값을 쓴다.
- `kr_roads::scale_round`/`scale_round_y2`/`round_up_to_even`/`round_or_omit`
  신설 — `round(실제_m × SCALE)` 계열 공식을 한 곳에 모았다.
- `kr_roads::section_spec(class, scale)`: 인도(2.0m, 짝수 올림)·갓길(2.5m)·
  차로(주간선 3.5m/보조 3.0m)·중앙분리대(2.0m, 물리형만) 전부 공식화.
  F등급의 "8(단일 포장면)"은 §1 어느 단위에도 대응하지 않는 값이라 실제
  4.5m로 역산해 disclosed 상수로 넣었다(`SPEC_RoadSection_v0.1.md §2`에
  이유 기록). §7 "축척 생략 규칙"(인도/갓길/중앙분리대 1블록 미만 →
  0, F 총폭 4블록 미만 → 전체 0)도 `round_or_omit`으로 구현 — 단,
  스케일=1.0/3.0 둘 다 이 임계값 아래로 안 떨어져서 실제 생략 발동은
  이번 검증에서 확인 못 했다(아래 결과 참고). 연석 폭(1, 고정)과
  연석 높이(`curb_height_y2`, `max(1, round(0.25m×SCALE×2))`)는
  §7 "생략 없음" 그대로 유지.
- `kr_roads::Segment`에 `scale: f64` 필드 추가 — 도로 배치 계열 함수
  (`sweep_and_place`/`register_ground_overrides`/`mark_paved_footprint`/
  `place_retaining_walls`/`place_segments`)가 세그먼트당 스케일을
  들고 다녀서, `data_processing.rs`의 여러 호출부를 전부 고칠 필요가
  없었다. `road_total_width`/`sidewalk_width`/`carriageway_half_width`/
  `furniture_column`/`carriage_edge_column`처럼 `Segment` 없이 `RoadClass`만
  받던 함수들은 `scale: f64`를 명시 인자로 추가.
- `kr_buildings::floor_heights(group, scale)`(주거·상가업무 3.4/4.6m,
  산업 6.9m)와 `total_height_blocks`도 스케일 인자 추가.
  `PlannedBuilding`에 `scale: f64` 필드 추가(facade.rs/slope.rs가
  `&PlannedBuilding`만 받아서 별도 인자 전달이 안 됨).
- `kr_buildings::slope`: §10.2 판정 경계(`grade_threshold`,
  `round(1.1m×SCALE)+1`)와 단일 축대 높이 상한(`max_single_tier_blocks`,
  R의 기준층 높이 재사용 — I를 포함해 모든 그룹에 하나만 적용, 그룹별로
  나누면 기존 동작이 바뀌므로 안 함)을 스케일화.
- `kr_street_furniture`: 전주 간격(34m 대표값)·높이(10.3m), 횡단보도
  길이(3.5m, "3~4m" 대표값)·정지선 폭(0.375m)·줄무늬 폭(0.45m) 전부
  공식화 — 정지선/줄무늬는 이전엔 암묵적으로 항상 1블록이던 걸 실제
  루프로 반복 가능하게 고쳤다(`stop_line_width`/`crosswalk_stripe_width`
  루프). 버스 승차대(폭·깊이·높이)는 §3.1 자체에 "실제" 열이 없어서
  기존 7×3×5 블록을 정확히 재현하는 실제값(4.0m/1.7m/2.9m)을 disclosed로
  붙였다(`SPEC_StreetFurniture_v0.1.md §3.1`에 표로 기록). 가로등·차선
  점선은 아직 미구현이라 대상 없음.
- SPEC_Bridge §2(세계 높이 제약)는 손댈 코드가 없었다 — 주탑/아치(§3/§4)가
  아직 구현 안 됐고(`bridges.rs`는 상판만 구현), 상판 높이는 이미 스케일이
  적용된 도로/지형 높이 계산을 그대로 쓰므로 별도 클램프 로직이 존재하지
  않는다. 확인만 하고 넘어감 — 나중에 주탑/아치를 구현할 때 이 세션이
  아니라 그때 §2를 적용해야 한다.
- `kr_bus_routes::REFERENCE_DATE`(고정 상수) 제거,
  `reference_date_from_filename(csv_path)`로 교체 — CSV 파일명
  (`..._YYYYMMDD.csv`)에서 직접 읽는다. CSV 자체엔 기준일 필드가 없어서
  내용이 아니라 파일명에서 읽는다; 패턴이 안 맞으면 기존 상수값으로
  경고와 함께 폴백. `PROGRESS.md §7` item 6 완료.

**2) scope/bbox 구조적 버그 (SCALE 검증 중 발견, `SPEC_Scope §0/§2` 위반)**

검증 1단계(P1 전역, rect-only vs rect∪508)에서 두 scope 설정의 타일 수가
399/462로 **동일**하게 나왔다 — route_strip을 넣으나 빼나 결과가 같다는
뜻으로, 사용자가 직접 진단했다: `--bbox`가 scope 조각들의 합집합과 무관한
독립 클립으로 남아 있어서, `--bbox` 밖으로 뻗는 route_strip 조각이 애초에
살아남을 수 없었다. `SPEC_Scope_v0.2.md §0/§2`는 "생성 범위 = 조각들의
합집합"이라고 명시하는데, `--bbox`는 그 합집합의 **입력**(사각형 조각을
만드는 재료) 중 하나여야지 그 위에 얹히는 별도 상한이면 안 된다.

**원인**: `build_transformer`의 `KoreaTm` 분기(`src/projection/mod.rs`)가
`XZBBox`(타일 그리드 전체)를 `korea_planar_bbox`(=`--bbox`/`--bbox-en`에서만
옴)의 `width_m()`/`height_m()`으로 산출한다 — scope가 나중에(`kr_transit`
안에서) resolve될 때는 이미 세계 크기가 고정된 뒤라, route_strip이 아무리
멀리 뻗어도 타일 그리드 자체가 거기까지 존재하지 않았다.

**수정**:
- `kr_scope::Scope::bounding_rect_en()` 신설 — 조각 전부(Rect는 4개 꼭짓점
  투영, RouteStrip은 각 점 ± `buffer_m`)의 EPSG:5186 외접 사각형을 낸다.
- `args::expand_bbox_for_kr_scope()` 신설, `main.rs`에서
  `apply_input_source_defaults` 직후·`args` 불변화 직전에 호출 — 이 시점의
  `korea_planar_bbox`로 `kr_transit::build_stops_document`를 미리 한 번
  돌려 `Scope`를 얻고, 그 `bounding_rect_en()`이 원래 `--bbox`보다 크면
  `args.bbox`/`korea_planar_bbox`를 그 외접 사각형으로 넓힌다. 매칭을
  한 번 더 하는 낭비(몇 초)를 감수하고 `generate_world_with_options`
  호출 그래프는 안 건드렸다 — `StopsDocument`를 앞으로 끌어와 재사용하는
  건 더 큰 리팩터라 이번엔 안 함.
- 확장 시 콘솔에 `KR scope: --bbox expanded from ... to ...` 로 알린다 —
  사용자가 지정한 범위가 조용히 넓어지는 걸 감추지 않는다.

**3) 검증 (지시대로 "실행 세 번", `RAYON_NUM_THREADS=3`)**

**1번 — 영도 P1 사각형, scale 1.75, diff 0**: 통과. 이전에 이미 얻어둔
"직전 커밋" 월드(`/c/mcarn_build/p1_baseline_prescale`, 커밋 `98c8ef65`
결과물)와 동일 설정으로 재생성한 새 월드를 대조했다.
- `roadgraph.json`/`buildings.json`/`stops.json`을 id 기준으로 정렬해
  비교 — **전부 0건 차이**. (raw MD5는 셋 다 달랐는데, Rust `HashMap`의
  랜덤 반복 순서 때문— 정렬 비교로 노이즈 제거했다. `buildings.json`만은
  MD5도 일치했다, `Vec` 기반이라 반복 순서가 원래 결정적.)
- Anvil 블록 레벨은 **전체 월드 exhaustive scan을 두 번 시도했다가 둘 다
  하네스 저메모리 보호에 죽었다** (약 436,000청크 × 전체 Y범위를 한
  Node 프로세스가 붙잡는 구조라 이 8GB 기계엔 안 맞음) — 재시도하지 않고
  방법을 바꿨다: 도로·건물(R/O/I 유형 포함) 실제 좌표 8곳을 표본으로
  뽑아 작은 bbox(200×200블록) 단위 `anvil-diff`를 순차 실행, 총 1,457
  청크에서 **차이 0**. 전수 스캔은 아니지만 위 JSON 동치 증명과 합쳐
  diff-0 결론에 신뢰도가 충분하다고 판단.
- `scripts/anvil-diff/semantic_diff.js` 신설(id 정렬 비교), 재사용 가능.

**2번 — scope 연결 실행 검증 (`§6`, 미뤄뒀던 것)**: 통과, 단 **P1
전역이 아니라 P0급 소규모로 다시 설계해서 검증했다** — 사용자가 위 구조적
버그를 진단하며 지시함. 두 번의 시행착오가 있었다:
- 1차 시도(P1 전역, rect vs rect∪508): 타일 수 399/462로 동일 →
  버그 발견(위 2번 항목). 재시도하지 않고 사용자에게 보고, 원인 진단과
  수정 지시를 받음.
- 2차 시도(수정 후, P0 사각형 35.0695~35.0829/129.0320~129.0484 ∪ 508):
  **508 자체가 부산 도심까지 뻗는 실제 장거리 노선**이라, rect를 아무리
  작게 잡아도 508의 실제 길이 때문에 고도 fetch 그리드가 P1급으로
  커져서 land-cover repair 단계에서 또 OOM. 재시도하지 않고 원인을
  파악(`--bbox expanded ... to 4308m x 6130m` 로그로 확인) —
  "짧은 노선 띠"가 필요하다는 지시를 다시 읽고 508 대신 **11번**
  노선으로 교체(정류소 이름이 전부 영도봉래시장·영도우체국·대교사거리·
  남항동·영선동뿐이라 확인된 영도 내 단거리 순환선, 실측 span 약 950m).
- 3차 시도(P0 ∪ 11번): **완주**. 타일 수 42(사각형만) vs 526/812
  (사각형∪11번) — **이제는 다르다**, 버그가 고쳐졌다는 직접 증거.
  road-continuity check 양쪽 다 0 unplaced.
- 사각형 구간이 "그대로 유지"됐는지는 **좌표 대조가 아니라
  link_id/building_id 집합 비교로 확인**했다 — 두 실행의
  `korea_planar_bbox` 원점이 다르고(확장된 실행은 서남단이 이동),
  더 결정적으로 **고도 압축 계수(`SPEC_Ingest §2.3`)가 scope 크기에
  따라 달라져서**(사각형만: compression 0, 최고 표고 157m / 사각형∪11:
  compression 0.029, 최고 표고 485m) 같은 실제 위치도 블록 Y가
  달라진다 — 이건 버그가 아니라 §2.3의 "H_LINEAR는 scope마다 자동
  산출" 설계가 의도한 그대로다. 그래서 좌표 오프셋을 맞춰도 Y까지는
  못 맞추니, 축척·압축과 무관한 **표준노드링크 link_id · 건물통합정보
  building_id**로 대조했다: 사각형만의 51개 세그먼트 link_id 전부
  사각형∪11번 쪽에 있음(0건 누락), 623개 건물 building_id도 전부
  있음(0건 누락) — 추가만 있고 손실 없음, 정확히 요구된 결과.
  `scripts/anvil-diff/offset-diff.js` 신설(원점이 다른 두 월드를
  정수 오프셋으로 맞춰 비교) — 결과적으로 이번엔 안 쓰였지만(고도
  압축 차이로 Y비교 자체가 무의미해서) 원점만 다르고 압축은 같은
  경우엔 유효하니 남겨둠.

**3번 — scale 1.0/3.0, 같은 P0급 사각형**: scale 3.0으로 시행, **완주**
(RAYON_NUM_THREADS=3, KR_SCOPE_TEST_SMALL 구성 그대로). 확인된 것:
- road-continuity check: 0 unplaced.
- §10.2 경사 판정 버킷이 로그에 그대로 찍혀 공식 적용을 실측 확인 —
  scale 1.75: `flat(≤2) retain(3-6) tiered(>6)` → scale 3.0:
  `flat(≤3) retain(4-10) tiered(>10)`. `report_slope_resolution`의
  `flat_max = round(1.1×3.0) = 3`(로그의 "flat(≤3)"과 일치, `slope::
  grade_threshold`는 이 값+1=4를 절토/축대 시작 경계로 쓴다 — 즉
  diff 4부터 그레이딩, diff 3까지 flat, 표기와 정확히 맞음).
  `max_single_tier_blocks(3.0)` = `floor_heights(R,3.0).1` =
  `round(3.4×3)=round(10.2)=10` → "tiered(>10)"과 일치.
- `roadgraph.json`의 실제 폭 값도 손으로 계산한 값과 정확히 일치:
  인도 6(=`round_up_to_even(round(2.0×3))`), D/E 차도 9
  (`round(3.0×3)`), C 18(×2차로), B 33(`round(3.5×3)=11`×3차로).
- 표현 문턱 생략(§7)은 **scale 1.0/3.0 둘 다 발동 안 함** — 인도가
  0으로 떨어지려면 scale이 0.25 미만이어야 해서, "합리적인" 축척
  범위에선 생략 규칙 자체를 실행으로 때려볼 수 없었다. 코드 로직은
  `round_or_omit`에 있고 §7 문턱값과 일치하게 짰지만, 이 세션에서
  실제 발동은 미확인 — 필요하면 scale <0.25 같은 비현실적인 값으로
  별도 확인해야 한다(생성 자체가 무의미해질 정도로 작은 값이라
  실익이 크지 않다고 판단해 하지 않음).

**메모리 교훈 (이 8GB 기계 한정)**: elevation fetch가 **scope 조각의
실제 지리적 span**(작은 rect라도 그 안에 들어간 route_strip이 멀리
뻗으면)에 따라 한 번에 큰 사각형 그리드를 통째로 잡는다 — "작은 테스트"를
설계할 때 rect 크기만 줄이는 걸로는 부족하고, **route_strip에 쓸 노선
자체가 짧아야** 한다. 508처럼 시내버스 간선(도심까지 관통)을 테스트용
노선으로 쓰면 rect가 아무리 작아도 P1급 그리드가 나온다.

---

**2026-09-19 상태 (1차) — P1 재생성 완주, 4개 검증 항목 모두 확인 완료:**

이번 세션에서 한 일 (코드, 빌드, 재생성 실행, 실행 결과 검증까지 전부 완료):

1. **도로 파묻힘 진단** — §4 항목 1의 수정(`bcdb7f6b` 2026-09-14 23:45, `b0da886f`
   2026-09-15 00:06)이 유일한 P1 전역 실행(§2, 2026-09-14, M4 확인용)보다 뒤에
   커밋됐고, `b0da886f` 자신의 커밋 메시지가 그 수정을 대교동/남항동 소구역과
   P0(영선동) 재실행으로만 검증했다고 밝히고 있다 — P1 전역 규모에서 재검증한
   적이 없다. 즉 사용자가 본 파묻힘은 그 수정이 있기 *전* 실행 결과일 가능성이
   높다 — **별도 수정 없이 재생성으로 확인하는 쪽으로 진행**. 아래 "검증 결과"
   1번에서 실행으로 확인됨(파묻힌 구간 0건) — 별도 수정 불필요했다는 결론이
   맞았다.
2. **유리창 비연결 수정** — `SPEC_BuildingType_v0.1.md §5.3`, 층마다 벽으로
   끊기는 반복 창(R-all, A-all, C-e1~e3, O-e2/e3)을 `glass_pane`에서 `glass`로.
   연속 띠창(C-e4, O-e4)은 `glass_pane` 유지. 구현: `kr_buildings/facade.rs`의
   `glass_for(group, era)` (기존 `glass_for(group)`에서 `era` 인자 추가).
3. **도로 위 식생 제거** — `SPEC_RoadSection_v0.1.md §5.1` 신설. 구현:
   `kr_roads::mark_paved_footprint`(신규 함수, 도로 단면 전체를 좌표 비트맵에
   등록 — 테이퍼 구간은 제외)를 `data_processing.rs`가 `kr_road_network` 계산
   직후 기존 `tunnel_footprint` 비트맵에 합쳐 넣고, `ground_generation.rs`의
   식생 배치 조건문에 `!tunnel_footprint.contains(x, z)`를 추가. 건물 오려내기와
   같은 "세운 뒤 지우지 않고 애초에 심지 않는다" 원칙 — `tunnel_footprint`는
   OSM 터널 전용이 아니라 범용 "여기엔 식생 금지" 비트맵이라 재사용했다(§ 관련
   근거는 `mark_paved_footprint`/`data_processing.rs`의 해당 지점 주석 참고).
4. **부수 발견 — `args.rs`의 stale 경고문 수정** — `--input-source kr`이
   `--mode terrain-only`를 강제하면서 "M1/M4가 아직 구현 안 됨"이라는, 이제는
   틀린 메시지를 띄우고 있었다. 실제로는 `GenerationMode`가 OSM/Overture
   객체 가져오기 여부만 통제하고(`skip_objects()`), kr_roads/kr_buildings/
   kr_street_furniture는 `args.mode`를 아예 참조하지 않는다 — 즉 이 강제
   자체는 무해하다(KR 입력은 애초에 OSM/Overture를 안 씀). 동작은 그대로 두고
   메시지만 정확하게 고쳤다. 코드 변경 자체는 아니지만 재빌드 필요(텍스트가
   컴파일된 바이너리 안에 있음).

**1차 시도(기본 스레드 수, 사실상 7) — 배치 단계 직후 하네스에 의해 강제 종료됨:**

`cargo build --release --no-default-features --target-dir /c/mcarn_build -j 2`
로 두 번 빌드(1차: 위 1~3 반영, 13분18초 / 2차: 4 반영, 19분36초) 모두 성공,
경고만 있고 에러 없음.

이어서 P1 bbox(`35.046,129.028,35.100,129.092`)로 실행 — 지형 계산까지
전부 새로 함(터레인 캐시 최초 미스), KR buildings 계산 완료(14843 로드,
12903 배치)까지 가고 `KR scope: L0 terrain buffer kept 399/462 tile(s)` →
**"Processing 399 tiles across 7 threads..." 직후, 실제 블록 배치가 시작되자마자
시스템이 메모리 부족으로 그 세션의 백그라운드 프로세스를 강제 종료했다**
(하네스 자체의 저메모리 보호 조치 — 명령 실패가 아니라 "세션이 유휴 상태일 때
시스템 메모리가 위험 수준"이라 잡힌 것). 이 시점 직전 여유 메모리는 ~2GB,
프로세스 자체 RSS는 최소 1.49GB까지 관측됨(총 8GB 중). 블록은 한 개도
안 놓인 채 죽어서 이 시도에서는 검증 결과가 전혀 안 나옴.

**2차 시도(`RAYON_NUM_THREADS=3`) — 완주, 241초, 종료 코드 0:**

사용자 확인 후 스레드 수를 낮춰 재시도(§7 아래 "3으로 먼저, 배치 단계에서
또 죽으면 2로" 지시). 1차 시도가 이미 터레인 캐시를 채워놨어서 이번엔
`[TERRAIN CACHE] hit`로 지형 재계산을 건너뛰었다 — 그래서 241초라는 총
소요 시간은 **스레드 수를 낮춘 효과만이 아니라 캐시 히트 효과가 섞여
있다**, 분리 안 됨(다음에 콜드 캐시로 전체 스코프를 돌릴 계획이 있으면
이 241초를 기준 삼지 말 것).

- **사용 스레드 수**: 3 (`RAYON_NUM_THREADS=3`). **완주 여부**: 완주,
  `EXIT_CODE:0`.
- **총 소요 시간**: 241초(약 4분) — 단, 터레인 캐시 히트 포함(위 참고).
- **배치 단계 피크 메모리(관측 가능한 범위)**: 10초 간격 샘플링(`tasklist`
  존재 확인 + `Get-CimInstance Win32_OperatingSystem`) 기준, 전체 실행 중
  최저 여유 메모리는 998MB(실행 시작 12초 시점 — KR 도로망 로드 구간으로
  보임, 표준노드링크 1,180,035 노드/1,557,364 링크를 읽는 단계), "Processing
  399 tiles" 배치 단계로 좁혀 보면 여유 메모리가 대략 1.4~2.2GB 사이(즉
  6~6.7GB/8GB 사용)를 오갔고 그 구간에서 하네스에 걸리지 않았다. 1차
  시도가 정확히 같은 배치 단계 진입 직후 죽은 것과 비교하면, 스레드 수를
  7→3으로 낮춘 것과 터레인 캐시 히트로 그 앞 단계(지형/지표피복/캐노피
  계산 — 대용량 그리드를 들고 있는 구간)의 기저 메모리 사용량이 줄어든
  것, 둘 다 기여했을 가능성이 높다 — 이번 한 번의 실행으로는 어느 쪽이
  결정적이었는지 분리할 수 없다.

**검증 결과 (4개 항목):**

1. **road-continuity check / 파묻힘 유무** — 프로그램 자체 출력:
   `KR roads: road-continuity check -- every centerline point has a placed
   road block.` 미배치 중심선 포인트 0개. **§4 항목 1의 진단이 실행으로
   확인됨** — `bcdb7f6b`/`b0da886f`의 수정은 지금까지 대교동/남항동
   소구역·P0(영선동)에서만 검증됐었는데, 이번에 P1 전역(851개 노드,
   1107개 세그먼트) 규모에서도 그대로 유지된다. 별도 수정 불필요했다는
   결론이 맞았다.
2. **도로 위 식생 제거** — `roadgraph.json`이 각 포인트마다 내보내는
   `road_half_width`/`curb_width`/`sidewalk_width`로 전체 포장 단면을
   재구성해(8포인트 간격 샘플링, 1107개 세그먼트 201,969개 포인트 중
   샘플) 460,445개 포장 컬럼을 직접 Anvil에서 확인(`scripts/anvil-diff/
   p1-verify.js`, 이번에 새로 작성) — **위반 70건(0.015%)**. 전부 도로
   경계 바로 바깥(포장 영역 밖)에 뿌리내린 나무의 **캐노피가 옆으로
   넘어와 덮은 것**(잎/통나무가 포장 표면 바로 위에 관측됨)이지, 나무
   자체가 포장 영역 안에 뿌리내린 사례는 0건이었다. §5.1이 막기로 한
   것("애초에 심지 않는다")은 **뿌리 위치** 기준이라 이 정의대로는
   완전히 지켜졌다 — 다만 인접 나무의 캐노피가 옆으로 침범하는 경우까지
   막지는 못한다는 게 이번에 드러난, 미처 명세에 없던 잔여 범위다
   (`SPEC_RoadSection_v0.1.md §8` "미정"에 남겨둠).
3. **건물 창면** — `buildings.json`의 `type_code`(그룹-시대)와 footprint로
   404개 건물 표본(전체 12,903개 배치 건물 중)의 창 행 블록을 확인 —
   **C-e4/O-e4는 100% `glass_pane`(115/187건, 0 `glass`)**로 띠창이
   설계대로 그대로 남았고, 그 외 표본에 나온 모든 유형(R-e1~e4, A-e1~e4,
   C-e1~e3, O-e2/e3, X-e1~e3)에서 `glass`가 관측됨(예: R-e2 871건,
   A-e2 643건, C-e2 733건). C-e1~e3/O-e2/e3에 남은 `glass_pane`(예:
   C-e2 454건)은 §5.2 1층 유리 전면(그룹별 규정, 이번 수정 대상 아님)과
   샘플링 Y범위가 겹친 것 — 버그 아님. I 그룹은 표본에서 창 자체가
   안 걸림(원래 창이 거의 없는 유형이라 6칸 표본으로는 못 잡을 수 있음,
   §4 "I는 창을 거의 두지 않음"과 일치하니 이상 없음).

**결론**: 4개 검증 항목 모두 이번 세션 수정이 의도대로 동작함을 확인.
유일한 잔여 항목은 위 2번의 "인접 캐노피 침범"(`SPEC_RoadSection_v0.1.md
§8` 참고, 필요 여부는 사용자 판단).

---

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

월드 검증은 Rust 안 거치고 Node(`prismarine-provider-anvil`/`prismarine-chunk`)로 저장된 Anvil 리전을 직접 읽는 스크립트를 매번 그때그때 짜서 했다. **2026-09-18부터는 아니다** — `scripts/anvil-diff/`에 재사용 가능한 버전을 남겼다(두 월드 폴더 + bbox를 받아 청크 단위 블록 diff 요약을 낸다; 합성 월드를 직접 만들어 실행하는 자체 self-test 포함, `npm test`). 다음에 Anvil 단계 검증이 필요하면 이걸 쓴다 — 새로 짜지 말 것. 사용법은 그 폴더의 `README.md`. 2026-09-19에 `p1-verify.js`를 같은 폴더에 추가했다 — `roadgraph.json`/`buildings.json`을 읽어 도로 포장 위 식생 잔존과 건물 창 블록 종류를 표본 검사한다(§0 2026-09-19 항목의 근거).

**메모리 상한은 빌드뿐 아니라 실행에도 있다 (2026-09-19).** 위 §0 항목 "빌드 시 메모리 부족"은 `cargo build`에 관한 것이고 별개다 — **P1(영도 전역) 규모로 `arnis.exe`를 실제로 돌릴 때도 같은 8GB 기계에서 메모리 상한에 걸린다.** 기본 스레드 수(rayon이 논리 코어 수 8에 맞춰 잡음, 로그에는 "7 threads"로 관측됨)로 돌리면 `Processing <N> tiles across 7 threads...`(M5까지 반영된 코드로는 전체 스코프 기준 399개 타일) 직후 블록 배치가 본격적으로 메모리를 잡기 시작하는 순간 하네스의 저메모리 보호 조치에 프로세스가 강제 종료됐다(2026-09-19 1차 시도, 여유 메모리 ~2GB 시점). **`RAYON_NUM_THREADS=3`으로 낮추면 완주한다**(2026-09-19 2차 시도, 241초, 배치 단계 여유 메모리 1.4~2.2GB 유지) — 단 이 비교는 터레인 캐시가 1차 시도에서 이미 채워진 상태로 진행돼서, 스레드 수 감소 효과와 캐시 히트로 그 앞 단계(지형/지표피복/캐노피 계산, 대용량 그리드 보유) 기저 메모리가 줄어든 효과가 섞여 있다 — 콜드 캐시(최초 실행, `--terrain-cache-dir`가 비어 있음)로 전체 스코프를 돌릴 때는 스레드 수를 낮춰도 지형 계산 단계 자체에서 다시 위험해질 수 있다는 뜻이니, 콜드 캐시 첫 실행이라면 처음부터 `RAYON_NUM_THREADS=3`(안전하면 2)으로 시작하고 실행 중 여유 메모리를 주기적으로 확인할 것. **권장**: 이 기계에서 P1 이상 규모를 돌릴 땐 기본값(7) 대신 `RAYON_NUM_THREADS=3`부터 시도하고, 배치 단계에서 또 죽으면 2로 낮춘다. CLI 자체에 스레드 수 플래그는 없다(`args.rs`에 없음) — 환경변수로만 조절 가능, 코드 변경 불필요.

---

## 6. 남은 과제

**M5 마저 구현** (§0 우선순위 순서):
1. §2/§3 커밋된 것 위에 uncommitted 횡단보도/정지선/팔레트 원복 — 빌드 확인 후 커밋 (§0 참고)
2. §4 가로등 (양쪽 교대 배치, 48블록 간격) — 미착수
3. 차선 점선 (3차로 이상) — `cross_section_layout` 확장 필요, 위 §3 참고
4. §6 교통시설(신호등·도로표지·볼라드), §7 기타(소화전·우편함·쓰레기통·맨홀), §5 가로수 — 전부 미착수. §8 우선순위표 전체(신호등이 전주보다 높음, 도로표지가 가로등보다 높음 등) 아직 부분적으로만 구현됐다는 뜻 — 나머지 요소 추가할 때 순서 다시 챙길 것.

**P1(영도 전역) 재생성** — M5 요소들이 들어간 채로 전체 스코프 한 번 다시 돌려서 완주 확인 안 함 (지금까지는 대교동/남항동 소구역 + P0 1.5km만 테스트).

**프리뷰 렌더러 자체 개선** — 지금은 도로 색을 실제 팔레트로 되돌리는 것으로 건물과의 충돌을 해결했지만(§4의 SPEC_RoadSection §3 도색이 실질적 구분 신호), `map_renderer.rs`가 `road_surface_overrides`를 참고해서 블록 색이 아니라 "이게 도로다"라는 사실 자체로 구분하게 만드는 게 근본적 수정이다. 지금은 안 건드림.

**scope 연결 실행 검증 — 완료 (2026-09-19), 단 예상과 다른 결과가 나와서 코드를 한 번 더 고쳤다.** 처음 계획대로 P1 전역에서 사각형만 vs 사각형∪508을 돌렸더니 **타일 수가 동일하게 나와** `Scope::contains_en`이 L0-L2에 연결은 됐지만 `--bbox` 자체가 scope 합집합과 무관한 독립 클립으로 남아 있던 별도 버그를 드러냈다 (§0 "2026-09-19 상태 (2차)"의 "2) scope/bbox 구조적 버그" 참고, `args::expand_bbox_for_kr_scope` 신설로 수정). 수정 후 P0급 소규모(사각형 ∪ 짧은 노선 11번, 508은 실제 거리가 길어 소규모 테스트에도 안 맞음)로 재검증: 타일 수가 달라짐(42 vs 526/812), link_id/building_id 집합 비교로 사각형 구간 무손실 확인(0건 누락), 508(원래 계획)이 아니라 11번으로 대체했다는 점과 좌표계 원점 차이 때문에 anvil-diff 블록 비교 대신 실측 id 비교를 썼다는 점이 원래 계획과 다르다 — 자세한 절차와 이유는 §0 항목 참고.

---

## 7. 일반화 계획 — 영도 말고 다른 지역에도 쓰려면

지금 코드는 여전히 영도구 하나만 놓고 만들어져 있다 — **이 절이 가리키는 코드 전수 조사는 2026-09-16 것 그대로다. 2026-09-18에 바뀐 건 설계(명세)뿐, 코드는 아직 손대지 않았다.** 아래 각 항목에 그 사이 확정된 명세 위치를 달아뒀다. 다음 세션은 이 명세를 따라 코드를 고치면 된다.

### 구조적으로 막힌 곳 (코드 수정 필요)

1. **완료 (2026-09-18, 커밋 `e456c168`).** `src/kr_transit/mod.rs`의 `PRIMARY_ROUTE`/`YEONGDO_LON_MIN/MAX`/`YEONGDO_LAT_MIN/MAX`/`is_in_yeongdo_range`를 전부 제거하고, 새 `src/kr_scope/mod.rs`(`Scope`/`ScopePiece`)의 scope 판정으로 대체했다. `kr_transit::resolve_scope_pieces`가 프리셋의 `ScopePieceInput`(`kr_scope::presets::yeongdo()`)을 실제 지오메트리로 바꾸고, 노선 채택·정류소 절단 둘 다 `Scope::contains_for_route` 하나로 판정한다.
   - **구현 중 명세가 한 번 더 갈렸다.** route_strip 조각을 모든 노선에 똑같이 적용되는 전역 판정(`Scope::contains`)에 썼더니, 508이 지나는 도심 환승 거점(남포동·중앙동·초량·부산역)을 스치기만 하는 무관한 노선까지 채택돼 노선 수가 20→51개로 늘었다(실측). `SPEC_Scope_v0.2.md §4.1.1` "조각의 두 역할"로 해소 — 영역 조각(rect/admin_polygon)은 모든 노선의 채택 판정에 쓰이고, 노선 조각(route_strip)은 **자기 노선의 판정에만** 관여한다. 지형·도로·건물 생성 범위(아직 코드에 안 붙어 있음)는 여전히 전 조각의 순수 합집합(`Scope::contains`)을 쓴다 — 예외는 노선/정류소 채택뿐이다.
   - **검증**: 영도 사각형 ∪ 508 route_strip 프리셋으로 실데이터(`data/`)를 돌려, 리팩터 전 스냅샷(`stops_review_BASELINE_pre_scope_refactor.json`)과 정규화 비교 — 완전 일치(20개 노선, 161개 정류소, route별 kept/cut까지 전부 동일).
   - **완료 (2026-09-18 연결 + 2026-09-19 실행 검증).** `Scope::contains_en`이 L0/L1/L2 생성 범위에 연결됐고, 2026-09-19 실행 검증에서 `--bbox`가 그 위에 별도 클립으로 남아있던 버그까지 찾아 고쳤다(`args::expand_bbox_for_kr_scope`, §6 "scope 연결 실행 검증" 참고). 이제 route_strip처럼 `--bbox` 밖으로 뻗는 조각도 실제로 생성 범위에 반영된다.

2. **`src/kr_roads/bridges.rs`의 `MANUAL_BRIDGES`가 영도대교·부산대교 두 개로 완전히 하드코딩돼 있다.**
   - 표준노드링크 데이터 자체에 교량 여부 필드가 없어서, 이 두 다리는 좌표(EPSG:5186 waypoints)와 MOCT 노드 ID를 손으로 찾아 박아넣은 것이다(M3 module doc에 이미 명시).
   - **스키마가 확정됐다.** `SPEC_Scope_v0.2.md §5.1`의 `[[bridges]]`가 그 설정 파일 형식이다 — waypoints, MOCT 노드 ID, 제외 링크 ID, 형식, 제원. `SPEC_Bridge_v0.1.md §0`도 "교량 목록은 프리셋 파일에서 온다"고 명시하도록 고쳤다. 하드코딩된 `&[ManualBridge]` 상수를 이 프리셋 필드를 읽는 코드로 바꾸면 된다. 여전히 사람이 표준노드링크를 뒤져서 교량 좌표를 찾아야 하는 것 자체는 못 피한다 — 그건 구조가 아니라 데이터 확보의 문제다.

### 확인은 필요하지만 구조는 괜찮은 곳

3. **Korea TM 투영(`src/projection/korea_tm.rs`)은 실제로 이미 일반적이다** — 원점(E0/N0)을 실행마다 `--bbox`/`--bbox-en`에서 계산한다, 하드코딩된 지역 좌표 없음. **서부/동부 TM 원점 자동 선택은 하지 않기로 결정했다** — `SPEC_Ingest_v0.1.md §2.1`에 이유를 명시했다: 전국 단일 좌표 프레임이라 인접 지역 맵이 이어 붙고, 국내 최원거리에서도 축척 오차 0.1% 미만이라 실용적 문제가 없다. EPSG:5186 하나로 고정하고, 다른 원점이 필요하면 프리셋 `overrides`로 덮어쓰는 것만 허용한다. 이 항목은 더 이상 "확인 필요"가 아니라 **결정 완료**다.
4. **완료 (2026-09-19).** `scale`이 `--input-source kr`에서 더 이상 고정 강제되지 않는다 — 기본값 1.75는 유지하되 `--scale`을 명시하면 그 값을 쓴다(`args::apply_input_source_defaults`가 `main.rs`의 `scale_explicit` 플래그로 "안 씀"과 "명시적으로 1.0을 씀"을 구분). `SPEC_RoadSection`/`SPEC_StreetFurniture`/`SPEC_BuildingType`의 치수 공식을 코드에 그대로 구현했다(`SPEC_Bridge §2`는 주탑/아치가 아직 미구현이라 대상 코드 자체가 없음 — 확인만 함). scale 1.75(diff 0)·3.0(완주, 임계값 실측 일치) 둘 다 검증됨, 자세한 내용은 §0 "2026-09-19 상태 (2차)" 참고. 이 작업 도중 scope/`--bbox` 관계의 별도 구조적 버그(§6과 얽힘)를 발견해 같이 고쳤다.
5. **`kr_buildings`의 건물통합정보 `.dbf` 컬럼 매핑(`A9`=주용도, `A13`=사용승인일 등)이 영도 실 데이터를 샘플링해서 역추적한 것이다** (컬럼명이 전부 익명화된 배포본이라 공식 필드 사전이 없다). **이제 이 매핑 자체를 설정으로 분리하는 구조가 정해졌다** — `SPEC_Scope §5.1`의 `buildings.dbf_schema`가 이름으로 가리키는 매핑 파일이다. 추가로 `SPEC_Ingest §4.1`이 적재 시 자기 검증(날짜 형식 확인, 용도 코드집합 대조, 실패 시 중단)을 필수로 요구하도록 바뀌었다 — 이게 있으면 다른 지역에서 매핑이 어긋나도 결측값으로 조용히 새는 대신 그 자리에서 멈춘다. 여전히 **데이터 검증 리스크**는 남는다 — 다른 지역 `.dbf`가 같은 스키마를 쓰는지는 실제로 넣어봐야 안다.
6. **완료 (2026-09-19).** `kr_bus_routes::REFERENCE_DATE`(고정 상수)를 없애고 `reference_date_from_filename(csv_path)`로 바꿨다 — CSV 파일명(`..._YYYYMMDD.csv`)에서 직접 읽는다(내용엔 기준일 필드가 없어서). 패턴이 안 맞는 파일명이면 기존 상수값(`2023-07-31`)으로 경고와 함께 폴백.
7. **CLI 플래그 자체(`--kr-bus-stops-dir` 등)는 이미 일반적**이다(임의 경로를 받음) — `args.rs`의 doc comment가 "부산 버스 정류소 SHP"라고 못박아 써놔서 다른 지역 데이터를 넣어도 되는지 헷갈릴 수 있다는 것뿐. 코드 문제 아니고 문서 문구 문제.

### 하지 않아도 되는 것

- `kr_roads`의 도로망 클리핑(`clip()`)은 이미 실행마다 넘어온 bbox로 동작한다 — 하드코딩 없음.
- `kr_street_furniture`의 모든 상수(간격·크기 등)는 지역 무관 설계 수치다 — `SPEC_StreetFurniture.md` 자체가 전국 공통으로 잡은 값들이고, 이제 축척 공식으로 재정의됐다(§4).
- `MOCT_LINK.dbf`/`MOCT_NODE.dbf` 파일명은 표준노드링크 자체의 전국 공통 스키마 — 손댈 필요 없음.

### 우선순위 제안

**1, 4, 6번 완료 (각각 2026-09-18/09-19/09-19).** 남은 건 2번(교량 프리셋 파일화)과 5, 7번 — 2번은 프리셋에 없으면 다리 없이 돌아가도록 설계됐으니(`SPEC_Bridge §0`) 급하지 않다. 3번(Korea TM 투영)은 결정 완료로 분류됨. 5, 7번은 실제로 다른 지역 데이터를 넣어보면서 하나씩 걸리는 대로 고치면 된다.
