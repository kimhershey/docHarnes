# DocuLens 평가 하네스 (`eval/`, P7)

설계 §3.11 "평가·품질 관리"의 골든셋 러너·지표·회귀 게이트입니다. 워크스페이스 bin 크레이트
`crates/doculens-eval`(실행 파일 `doculens-eval`)로 구현했고, 이 폴더에는 골든셋·기준선·매니페스트 예시가 있습니다.

```
eval/
  golden/planner.json                  플래너 골든셋 129건(0.7.0 기준: 한국어 93·영어 26·혼합 10)
  golden/planner-context.json          골든셋 실행에 쓰는 PlanContext(기준일 2026-11-08, 문서 유형·스키마·폴더·태그·모델·프로젝트)
  golden/pipeline-manifest.example.json 파이프라인 골든 매니페스트 예시(gen_fixtures 합성 샘플 기준)
  baselines/planner-rules.json         규칙+프리라우터 경로의 현재 정확도 기준선(CI 게이트가 비교)
```

## 설계와의 차이: Python → Rust

설계(§3.2 BOM, §4.1 P7)는 `eval/`를 Python 전용(jiwer·anls_star 등)으로 그렸습니다. 이 저장소는 **Rust 워크스페이스 크레이트**로
구현합니다. 이유:

- 툴체인이 하나(cargo)라 사용자 PC·CI 어디서나 `cargo run -p doculens-eval`로 동일하게 돈다 — Python 설치·가상환경이 필요 없다.
- 라이선스 게이트가 같다(`cargo deny check licenses bans sources`). 외부 의존성을 추가하지 않고 워크스페이스에 이미 있는
  `serde`·`serde_json`·`chrono`·`doculens-core`만 쓴다(설계가 금지한 GPL `Levenshtein`/`distance` 류 문제가 원천적으로 없음).
- 플래너·파이프라인·정규화기를 **제품 코드 그대로**(`doculens_core::planner::llm::plan`, `pipeline::run::process`,
  `pipeline::validate::{normalize_amount, normalize_date, brn_digits}`) 호출하므로 평가와 제품의 동작이 어긋나지 않는다.
- 배포 대상이 아니므로(`publish = false`, 설치본에 포함되지 않음) 설계의 "미배포" 조건은 그대로다.

TEDS/TEDS-S(표 구조)와 근거 bbox IoU는 이번 단계에 포함하지 않았습니다(아래 "남은 일").

## 빌드·실행

```bash
cargo test -p doculens-eval                              # 지표·골든셋 비교 단위 테스트
cargo run -p doculens-eval -- --help
```

### 1. 플래너 정확도 (`planner`)

```bash
# 규칙 + 프리라우터만(결정적, 모델 불필요) — CI 게이트와 같은 명령
cargo run -p doculens-eval -- planner --golden eval/golden/planner.json \
    --baseline eval/baselines/planner-rules.json --out eval/out/planner-rules.json

# 앱의 기본 해석과 같은 모델 우선 경로(슬래시 명령만 프리라우터, 나머지는 모델 + 재질의 1회)까지: 실제 llama-server + 드롭인 모델 폴더
cargo run -p doculens-eval -- planner --golden eval/golden/planner.json \
    --engine <llama-server 경로> --models-dir <.doculens/models> --model gemma4-e4b-q5km \
    [--backend cpu|vulkan|cu121|cu129] [--profile balanced] --out eval/out/planner-gemma4-e4b-q5km.json
```

<!-- 0.8.0: 리드가 갱신 -->
- 모든 골든 명령을 `doculens_core::planner::llm::plan(None, text, &ctx, PlanMode::RulesFirst)`로 돌립니다(프리라우터 → 문장 규칙;
  규칙이 못 풀면 `no_model` 명확화로 끝남). 이것이 CI 게이트가 재는 **규칙 경로**이고, 앱에서는 해석을 "규칙(구형)"으로 바꿨을 때의
  동작입니다. `--engine`이 있으면 `plan(Some(&PlannerDeps{..}), …, PlanMode::ModelFirst)` — 앱의 기본 해석인 **모델 우선 경로**
  (슬래시 명령만 프리라우터, 그 밖의 문장은 모두 모델, 검증 실패 시 재질의 1회) — 를 한 번 더 돌려 두 결과를 모두 보고하고, 결과
  파일의 1차 값은 모델 경로가 됩니다(`rules_only`에 규칙 결과 동봉). 모델 경로의 정확도는 CI에서 재지 않습니다(GPU·모델 필요).
- 컨텍스트는 골든 파일의 `"context"`(골든 파일 기준 상대 경로) 또는 `--context <json>`; `PlanContext` 구조 그대로
  JSON으로 적습니다(`today`, `language`, `folders`, `doc_types`, `schemas`, `tags`, `current_folder`, `selected_doc_ids`,
  `current_document`, `current_page`, `document_count`, `model_ids`, `projects`, `current_project`).
- 출력: 전체·카테고리별(`ko`/`en`/`mixed`)·의도별 정확도, 플랜 출처(prerouter/rules/model/none) 분포, 불일치 전건을
  **원인별로 묶어** 기대 플랜 / 산출 플랜 요약과 함께 출력합니다. `--verbose`는 항목마다 한 줄씩 찍습니다.
- `--out`은 결과 JSON(항목별 정오·원인·산출 연산·범위·지연 시간·토큰 수)을 씁니다. 이 파일이 다음 기준선이 됩니다.

#### 골든 항목 형식과 판정 규칙

```json
{ "id": "ko-03", "category": "ko", "text": "지난달 견적서 합계 얼마야?",
  "expected": { "intent": "aggregate", "ops": ["aggregate"],
                "params": [ { "fn": "sum", "field": "grand_total" } ],
                "scope":  { "filter": { "doc_type": "estimate", "date_from": "2026-10-01", "date_to": "2026-10-31" } } },
  "note": "설계 예시 3" }
```

- **정답 판정**: `intent`가 같고, 연산 종류(`op`)의 **순서 있는 목록**이 `expected.ops`와 같고, `expected.params[i]`(선택,
  연산별·`null`은 미검사)가 직렬화한 연산(`serde_json::to_value(&op)`)의 **부분집합**이고(객체는 재귀, 배열은 길이·원소 일치,
  값 `null`은 "그 파라미터가 없어야 함"), `expected.scope`(선택)가 직렬화한 범위의 부분집합이면 정답입니다.
  파라미터 이름·값은 부록 D(`planner/schema.rs`의 `OP_SPECS`)의 직렬화 이름을 씁니다(`fn`, `ref`, `theme: "high-contrast"` …).
- 항목에 `"context": { … }`를 주면 그 항목만 공용 컨텍스트의 **최상위 키를 덮어써서** 실행합니다(예: 비교 명령에
  `{"selected_doc_ids": ["d1a2", "d7f3"]}` — 선택이 없는 컨텍스트에서는 "2건 이상 선택하세요" 되묻기가 정답이므로).
- 기대값은 **규칙 출력이 아니라 사람이 기대하는 플랜**입니다. 그래서 규칙 경로의 정확도는 100%가 아니며, 불일치 목록이 규칙
  개선 작업 목록이 됩니다. 일부 항목은 의도적으로 경계 사례입니다(`ko-32` 열린 견적서에 대한 질문은 집계가 아니라 답변,
  `ko-19` 잡담은 `noop`, `ko-70` 대상 없는 삭제는 `clarify`).
- **원인 분류**(결과 JSON `cause`, 보고서 묶음): `no_rule` 규칙 없음(모델 필요) · `rule_clarified` 규칙이 되묻기로 후퇴 ·
  `over_interpretation` 과잉 해석(되묻기·무시가 정답) · `wrong_ops` 의도·연산 오판 · `params` 파라미터 누락·오류 ·
  `scope` 범위 오류.
- 덮는 연산 종류(0.7.0 기준): `OpKind::ALL` 63종 가운데 59종. `manage_template`·`merge`·`remove_from_project`·`workspace`는 아직
  골든 항목이 없습니다. 파라미터는 `expected.params`를 적은 항목만 검사하므로 연산 종류만 채점하는 항목이 섞여 있습니다(P6에서
  들어온 `draft_document`·`define_rule`·`check_consistency` 등의 항목은 대부분 그렇습니다). 설계 예시 명령 12종(`docs/user-pc-checklist.md` P5)은 모두
  포함되어 있습니다(`note: "설계 예시 n"`). <!-- 0.8.0: 리드가 갱신 -->

#### 회귀 게이트

`--baseline <results.json>`을 주면 그 파일의 1차 정확도와 비교해 **0.5 pt 초과 하락이면 종료 코드 1**(§3.11 "0.5pt 이상 하락 시
차단")로 끝나고, 새로 실패한 항목·새로 통과한 항목을 나열합니다. 기준선과 현재의 모드(`rules`/`model`)가 다르면 경고합니다.
CI(`.github/workflows/ci.yml`의 `eval-gate`)는 규칙 경로를 `eval/baselines/planner-rules.json`과 비교합니다.
규칙을 개선해 정확도가 올라가면 `--out eval/baselines/planner-rules.json`으로 기준선을 올려 커밋하세요(내려가는 기준선은 PR에서
이유를 적을 것).

### 2. 파이프라인 지표 (`pipeline`)

```bash
# 매니페스트·파일 확인만(엔진 불필요) — 지표 섹션은 "미실행"
cargo run -p doculens-eval -- pipeline --manifest eval/golden/pipeline-manifest.example.json --out eval/out/pipe-dry --dry-run

# 실제 실행: 평가용 워크스페이스를 만들고 가져오기 → 분류 → OCR → 추출 → 채점
cargo run -p doculens-eval -- pipeline --manifest <골든 매니페스트.json> --out eval/out/gemma4-e4b-q5km \
    --engine <llama-server> --models-dir <.doculens/models> --model gemma4-e4b-q5km \
    [--pdfium <libpdfium.so|pdfium.dll|폴더>] [--backend cu121] [--profile accuracy] [--precise] [--limit 5] [--verbose]
```

흐름: `--out/workspace`(또는 `--workspace <dir>`)에 DocuLens 워크스페이스를 만들고(`Workspace::open(create)` + 내장 양식
`ensure_builtins`), 매니페스트 문서를 **복사·렌더**해 가져온 뒤(`documents::import::{plan, run}`, PDF는 PDFium 필요 — `--pdfium`,
`DOCULENS_PDFIUM`, 번들/AppData 순으로 탐색; 이미지는 PDFium 없이 렌더; HWPX/DOCX는 `StubPdfPrinter`로 변환), 엔진을 띄워
(`EngineManager`, 프리셋 자동 생성, `--list-devices`로 백엔드·티어 결정) 문서마다
`pipeline::run::process(level=extract, force=true, pages=매니페스트 pages)`를 실행하고 DB에서 결과를 읽어 채점합니다.
추출 미러는 `<workspace>/.doculens/extractions/<doc>/<run>/result.json`에 남습니다.

#### 매니페스트 형식

```json
{ "schema_version": 1, "name": "설명(선택)", "strict_keys": false,
  "documents": [
    { "path": "relative/or/absolute.pdf",          // 매니페스트 파일 기준 상대 경로 또는 절대 경로
      "doc_type": "tax_invoice",                   // 기대 문서 유형 슬러그(분류 정답)
      "fields": { "supply_total": "1000000", "issue_date": "2026-10-14", "supplier.brn": "123-45-67890", "remarks": null },
      "text": "optional expected OCR text",        // 채점 대상 페이지의 기대 텍스트(선택)
      "pages": [1] } ] }                           // OCR·추출·채점 대상 쪽(비우면 전체)
```

- `fields`의 키는 **필드 경로**(`supply_total`, `supplier.brn`, `items[0].amount`)이고 값은 문자열·숫자, `null`은 "비어 있어야
  함"입니다.
- `strict_keys: true`면 매니페스트에 없는 키로 비어 있지 않은 값이 나오면 환각 키로 세어 ANLS* 분모에 더합니다(스키마의 모든 키를
  적은 **완전 라벨링**일 때). 기본 `false`는 부분 라벨링 — 적지 않은 키는 채점하지 않습니다(환각 수는 집계만).
- `eval/golden/pipeline-manifest.example.json`은 `DOCULENS_FIXTURE_OUT=eval/fixtures cargo test -p doculens-core --lib -- --ignored gen_fixtures`
  로 만든 합성 샘플 4종을 가리킵니다(`eval/fixtures/`는 git 무시). 값은 형식 예시일 뿐 실제 정답이 아닙니다.

#### 골든셋 만들기(라벨링)

§3.11대로 **검토 큐가 라벨링 도구**입니다: 문서를 가져와 자동 처리 → 검토 화면에서 필드를 수정·승인 → 승인된 값이 정답.
"골든셋으로 내보내기" UI는 아직 없으므로 지금은 다음 절차로 매니페스트를 손으로 조립합니다.

1. 검토가 끝난 문서의 추출 미러 `.doculens/extractions/<doc_id>/<run_id>/result.json`(`LATEST` 파일이 최신 run id)을 엽니다.
2. `fields[]`의 `path`를 키로, 검토 후 값을 값으로 옮깁니다(`value`는 추출값; 검토에서 고친 값은 DB `fields.value`/`original_value`
   — 내보내기(`export` XLSX/JSONL)의 검토 완료 값을 쓰면 편합니다). 비어 있어야 하는 필드는 `null`.
3. `document.doc_type`을 `doc_type`에, 사람이 확인한 전사 텍스트가 있으면 `text`에(없으면 생략 — CER은 그 문서를 건너뜀).
4. 원본 파일을 마스킹한 사본과 함께 한 폴더에 두고 `path`를 상대 경로로 적습니다. 문서유형별 ≥50쪽(실제 스캔·사진·디지털)이
   §3.11의 권장량입니다.

#### 지표 정의

| 지표 | 정의 |
|---|---|
| 문서 유형 분류 정확도 / macro F1 | 문서 단위 `doc_type` 일치율; 정답∪예측에 나온 라벨마다 F1을 구해 **가중 없이 평균**(한 번도 안 나온 라벨은 제외). |
| OCR CER | 채점 대상 페이지 텍스트를 이어 붙여(`pages.text`, 텍스트 레이어/변환/OCR 출처 모두) 기대 텍스트와 비교. 양쪽 **NFC 정규화 + 공백 연속 → 한 칸 + 양끝 공백 제거** 후 문자 단위 Levenshtein 거리 ÷ 기대 길이. 문서 평균을 보고. |
| 필드 정확일치율 | 기대 키마다 **종류별 정규화** 후 문자열 일치: 금액·정수 `normalize_amount`(`1,250,000원`→`1250000`), 날짜 `normalize_date`(→`YYYY-MM-DD`), 사업자번호 `brn_digits`(10자리), 전화 `normalize_phone`, 그 외 NFC+공백 정규화. 종류는 산출 필드의 `kind`를 따르고, 산출이 없으면 불일치. `null` 기대는 산출이 비어 있을 때 일치. 전체 기대 키 수 대비 일치 수(마이크로). |
| ANLS* | 문서마다 기대 키별로 정규화 문자열의 유사도 `1 − 편집거리/최대길이`를 **0.5 임계**로 자른 값(미만은 0)의 평균. 누락 키 0, `null` 기대인데 값이 나오면 0, `strict_keys`면 환각 키가 분모에 더해짐. 데이터셋 값은 문서 평균. |
| 페이지 처리 시간 p50/p95 | 문서 한 건의 `process` 벽시계 시간 ÷ 채점 쪽수를 쪽마다 표본으로 두고 최근접 순위 백분위수. 분류·OCR·추출을 모두 포함. |
| VRAM | 엔진의 `--list-devices`가 보고한 장치별 `전체 − 여유`를 로드 직후와 문서마다 표집해 로드 후 사용량과 **피크**를 기록(다른 프로세스 사용분 포함). CPU 빌드는 비움. `engine.model_memory_bytes`는 라우터가 보고한 모델 메모리 + mmproj 크기. |

출력: `<out>/results.json`(문서별 상세·필드별 정규화 전후 값·환경 정보)과 `<out>/scorecard.md`(한국어 표; `--dry-run`이면
지표 섹션이 **미실행**).

### 3. 양자화 스코어카드 (`scorecard`)

```bash
cargo run -p doculens-eval -- scorecard \
    --inputs eval/out/q4km/results.json eval/out/q5km/results.json eval/out/q6k/results.json eval/out/planner-q4km.json \
    --out docs/scorecard.md [--title "양자화 스코어카드 — RTX 4060"]
```

여러 `results.json`(모델·양자화·엔진 빌드마다 하나; `pipeline` 결과와 `planner` 결과 모두 가능)을 한 표로 합칩니다:
모델 · 양자화(결과의 `quantization`, 없으면 모델 id의 `q4_k_m`·`q6k` 같은 토큰에서 추정) · 엔진(백엔드·빌드) · 장치 · 분류 F1 ·
분류 정확도 · 필드 정확일치 · ANLS* · OCR CER · 페이지 p50/p95 · VRAM 피크 · 플래너 정확도. `docs/scorecard.md`는 이 표의 빈
템플릿이며 이 명령으로 덮어씁니다.

## 종료 코드

`0` 성공 · `1` 회귀 게이트 실패 · `2` 인자/입출력 오류(매니페스트·골든셋 검증 실패 포함).

## 남은 일

- TEDS/TEDS-S(표 구조 유사도)와 근거 bbox IoU≥0.5 비율(§3.11) — 표 추출 결과(`ExtractedTable`)와 필드 `bbox`를 쓰는 지표 추가.
- "골든셋으로 내보내기"(검토 큐 → 매니페스트) UI/커맨드.
- 파이프라인 지표의 기준선 비교(`pipeline --baseline`) — 지금은 스코어카드 표에서 수동 비교.
- 실제 Gemma 4 E4B GGUF와 사용자 골든셋으로 측정한 값은 사용자 PC에서 채웁니다(`docs/scorecard.md`).
