# McARN

오픈소스 Arnis(github.com/louis-e/arnis) 포크(패키지 `arnis`, 바이너리 `arnis.exe`). `--input-source kr`로 한국 공공 GIS 데이터(표준노드링크 도로망·건물통합정보·부산 버스)를 읽어 실제 지형·도로·건물을 재현한 Minecraft 월드를 만든다. 1차 대상은 부산 영도구.

## 명세 문서
- 진행 상황·다음 할 일: `docs/PROGRESS.md` — 다음 세션은 이것 하나로 이어간다(§0 지금 할 일, §5 빌드, §6 남은 과제).
- 명세: `docs/SPEC_*.md`. 마일스톤 정의 `SPEC_Build_v0.1.md`, 범위 `SPEC_Scope_v0.2.md`(`SPEC_GenerationScope_v0.1.md`는 폐기), 검증 지표 `SPEC_Validation_v0.1.md`.
- 원본 Arnis 문서: `README.md`(영문, upstream 것).

## 빌드·테스트·실행
- 빌드(Windows, `PROGRESS.md` §5): NASM을 PATH에 넣고
  `cargo build --release --no-default-features --target-dir /c/mcarn_build` → `/c/mcarn_build/release/arnis.exe`.
  `--target-dir`은 ASCII 경로, `--no-default-features`로 GUI를 끈다. 메모리 부족 대처도 §5.
- 테스트(CI와 같은 것): `cargo fmt -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-targets --all-features`.
- 월드(Anvil) 검증: `scripts/anvil-diff/`(사용법은 그 폴더 `README.md`, 자체 시험 `npm test`).
- 실행 예시(P0 영선동): `PROGRESS.md` §5. P1 이상 규모는 `RAYON_NUM_THREADS=3`부터(§5).

## 지켜야 할 규칙
- Anvil 단계 검증은 `scripts/anvil-diff/`를 쓴다 — 새로 짜지 않는다(`PROGRESS.md` §5).
- `PROGRESS.md`는 그 문서 하나만 읽고 다음 세션을 이어갈 수 있게 갱신한다.
- 원격: `origin` = jinsu0947-hub/McARN, `upstream` = louis-e/arnis.

## 공통 규칙

이 저장소의 규칙이 공통 규칙과 다르면 저장소 규칙을 따른다.

### Git
- 새 프로젝트를 만들면 바로 Git 저장소로 만들고, `jinsu0947-hub`에 비공개(private)로 올린다.
- 폴더를 복사해서 사본을 만들지 않는다. 되돌릴 일은 Git 기록으로 한다.
- 작업 단위가 끝날 때마다 커밋하고 푸시한다.
- 작업을 시작하기 전에 git pull로 GitHub 최신본을 받는다.
- main이 아닌 브랜치(예: claude/로 시작하는 것)가 GitHub에 있으면 작업 전에 알려준다.

### 확인이 필요한 작업
- 파일 삭제와 Git 기록 되돌리기는 하기 전에 나에게 묻는다.

### 설계 결정
- 새로 정해진 설계 결정은 그 프로젝트의 CLAUDE.md나 docs에 적는다.
- 방향·기능·범위 결정은 내가 한다. 기술적 판단은 스스로 내리고 그 이유를 기록한다.

### 보고
- 보고는 한국어로, 짧게 한다.
