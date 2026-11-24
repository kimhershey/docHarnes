# 플래너 — 실제 모델 측정

`doculens-eval planner --engine …`로 실제 모델이 쓴 계획을 잰 기록입니다. 1.0.2까지의 측정 결과와 문장 세트는 1.0.4에서 지웠습니다(git 기록에 있습니다).

| | |
|---|---|
| 모델 | Gemma 4 E4B Q4_K_M (`gemma4-e4b-q4km`, 레지스트리의 SHA-256과 일치하는 파일) |
| 엔진 | llama-server, CPU 4코어, 컨텍스트 8,192 |

## 1.0.3 — 요약은 결과 하나: 문서별 요약 없음 (사용자 요청 2026-11-22)

"파일별 요약은 없애고 모든 파일의 내용을 하나의 결과로 요약" — `summarize.target`(each_doc · set)이 없어졌고, 요약 단계는 범위의 문서 전체를 읽어 결과 하나를 만듭니다.
계획 쪽에서 볼 것은 세 가지였습니다: 모델이 없어진 칸 없이도 요약을 계획하는가, 문서마다 따로 요약해 달라는 문장을 어떻게 쓰는가, 문장이 청한 묶음(프로젝트별)은 남는가.

### 문장 12개 (`one-summary-1.0.3.golden.json`) — 9/12 → 12/12

컨텍스트는 1.0.2의 `topic-summary-1.0.2-open-document.context.json`(문서 하나가 열려 있고, 프로젝트 "한빛 납품"과 "AAA"가 있음)입니다.

| id | 문장 | 가드 전 | 이 판 |
|---|---|---|---|
| s-1 | 전체 문서를 요약해줘 | ✕ `{length: long}` | ○ `{}` |
| s-2 | 이 문서 요약해줘 | ○ | ○ 열린 문서 |
| s-3 | AAA 프로젝트 요약해줘 | ○ | ○ 프로젝트 AAA(참고 자료 포함) |
| s-4 | 이 문서를 한 줄로 요약해줘 | ○ `{length: short}` | ○ |
| s-5 | 전체 문서를 문서별로 요약해줘 | ○ 확인 카드 | ○ 확인 카드 |
| s-6 | AAA 프로젝트 문서를 각각 요약해줘 | ○ 확인 카드 | ○ 확인 카드 |
| s-7 | 파일별로 요약해줘 | ○ 확인 카드 | ○ 확인 카드 |
| s-8 | 프로젝트별로 요약해줘 | ○ `{group_by: project}` | ○ |
| s-9 | AAA에 대해서 요약해줘 | ○ `{topic: AAA}` | ○ |
| s-10 | AAA 프로젝트의 지체상금 내용에 대해 요약해줘 | ○ `{topic: 지체상금}` · 프로젝트 AAA | ○ |
| s-11 | 모든 문서의 내용을 하나로 요약해줘 | ✕ `{length: long}` | ○ `{}` |
| s-12 | summarize all documents | ✕ `{length: long}` | ○ `{}` |

- 열두 문장 모두 한 번에(재질의 없이) 요약 계획이 나왔고, 없어진 `target`을 쓰려다 문법에 걸린 경우는 없었습니다.
- **길이** — "전체 · 모든 · all"이 들어간 세 문장에 모델이 `length: long`을 붙였습니다. 문장은 길이를 말하지 않았고, `long`은 여러 쪽 분량의 서술식 요약이어서 CPU에서 몇 분이 더 걸립니다.
  `guard::unsaid_length`(검증 전): 요약의 길이는 문장이 말한 대로 — 말하지 않았으면 비우고, "자세히" · "한 줄로"는 계획이 무엇을 썼든 넣습니다. 주제의 낱말("상세 설계에 대해")은 길이가 아닙니다.
  첫 측정은 길이를 기대값에 넣기 전이어서 결과 파일 안의 정답 수는 12/12이고, 위 "가드 전" 열은 길이를 넣은 기대값으로 다시 채점한 것입니다
  (`python3 scripts/rescore-planner.py eval/results/planner-real-model/one-summary-1.0.3.golden.json 가드전=…-before-guards.json 이판=….json`).
- **문서별 요약 요청**(s-5 · s-6 · s-7) — 모델은 세 번 모두 묶음 없는 `summarize`를 썼고 `guard::split_check`가 "문서별 요약은 만들지 않습니다 …"를 붙여 확인 카드가 됐습니다.
  모델이 쓴 해석 문장은 "전체 문서의 내용을 문서별로 요약합니다."(s-5), "파일별로 문서를 묶어 요약합니다"(s-7)여서 카드의 알림과 어긋났습니다.
  이 판은 그런 해석 문장을 "범위의 문서를 모두 읽어 하나로 요약합니다"로 바꿉니다(프로젝트 이름을 말한 s-6의 "'AAA' 프로젝트의 문서를 요약합니다"는 그대로).
- 응답 시간은 문장당 18 ~ 164초였습니다(CPU 4코어, 1.0.2와 같은 범위).
- 잰 것은 계획까지입니다. 여러 문서를 하나로 요약한 결과물은 `crates/doculens-core/tests/real_summary.rs`(`DOCULENS_SUMMARY_TOGETHER=1`)로 따로 돌렸습니다 — `docs/releases/v1.0.3.md` "확인한 것".
  E2B · GPU는 재지 않았습니다.

## 1.0.5 — 문장이 말하지 않은 범위 조건 · "알려줘" · 유형 조건 · 선택지 (사용자 보고 2026-11-24)

**프로젝트 + 참고 자료 (8문장, `project-reference-1.0.5.golden.json`)** — 사용자 보고 "프로젝트명 명시 시 참고자료 범위를 지정할 수 없음"을 재현하려고 잰 것입니다.

| 문장 | 고치기 전 범위 / 동작 | 고친 뒤 |
|---|---|---|
| AAA 참고자료 요약해줘 | `project_id` · `role` + **`status: reviewed`** | `project_id` · `role: knowledge` |
| 한빛 납품 프로젝트의 참고 자료를 요약해줘 | + **`tags: ["한빛 납품"]`** | 같음 |
| AAA 프로젝트의 참고자료에서 보안 요구사항 알려줘 | + **`tags: ["AAA"]`** | 같음 (answer) |
| AAA프로젝트 참고자료 요약 | + **`tags: ["AAA"]`** | 같음 |
| AAA 프로젝트의 참고자료에서 요구사항 정의서 요약해줘 | **`manage_reference add` + summarize(`group_by: project`)**, 범위 `doc_type: srs` · `tags: ["knowledge"]`, 프로젝트 없음 | summarize, `doc_type: srs` · `project_id` · `role: knowledge` |
| 나머지 세 문장 | 맞음 | 맞음 |

- 고치기 전 3/8(모델이 쓴 조건을 그대로 두면 범위가 0건이 되어 "참고 자료가 없습니다"로 실패), 고친 뒤 **8/8** (`guard::unsaid_scope`). 문장당 24 ~ 143초.
- 고치기 전 결과 파일의 정답 표시는 기대값을 채우기 전 것이라 쓰지 않습니다(위 표는 범위와 동작을 직접 비교한 것).

**"알려줘" · 유형 조건 · 선택지 (골든셋 11문장, `--ids ko-31,ko-140,ko-142,ko-154,…,ko-161`)** — **11/11**. 문장당 19 ~ 133초.

- "납품 기한 알려줘", "검수 기준에 대해 알려줘", "계약서에서 해지 조건 알려줘" → `answer`(유형 조건 없음). "한빛 납품 프로젝트의 검수 기준에 대해 알려줘", "이 문서에서 지체상금에 대해 알려줘" → 그 범위의 주제 요약.
- "견적서에 대해 요약해줘", "회의록에 대해 알려줘" → 선택지 `type` / `topic`. "한빛 납품에 대해서 요약해줘", "한빛 납품에 대해 알려줘" → 선택지 `project` / `topic` / `project_topic`. "계약서 해지 조건에 대해 요약해줘"는 묻지 않음(반례).
- `ko-31` "'위약금' 들어간 계약서 찾아줘"는 첫 측정에서 모델이 `open`(문서, ref "위약금")을 써서 틀렸습니다. `guard::find_is_search`를 넣고 그 문장만 다시 재서 `search`(범위 조건 없음)가 됐습니다. 결과 파일의 그 항목에 첫 결과를 `first_measurement`로 남겼습니다.

읽는 법: 잰 것은 계획까지입니다. 선택지를 고른 뒤의 실행, 유형 겹침 선택지(DB가 필요)는 이 측정에 없습니다. E2B · GPU는 재지 않았습니다.

## 파일

- `one-summary-1.0.3.golden.json` — 1.0.3의 12문장. 컨텍스트는 `topic-summary-1.0.2-open-document.context.json`.
- `project-reference-1.0.5.golden.json` — 1.0.5의 8문장(컨텍스트는 같은 파일). `gemma4-e4b-q4km-1.0.5-project-reference-before.json` / `…-project-reference.json` — 고치기 전과 후.
- `gemma4-e4b-q4km-1.0.5-tell-type-choices.json` — 골든셋 11문장(문장은 `eval/golden/planner.json`).
- `one-summary-1.0.3.golden.json`의 s-9("AAA에 대해서 요약해줘")는 1.0.5에서 기대값이 선택지 세 개의 되묻기로 바뀌었습니다(1.0.3 결과 파일은 그대로).
- `gemma4-e4b-q4km-1.0.3-one-summary-before-guards.json`, `gemma4-e4b-q4km-1.0.3-one-summary.json` — 12문장: `guard::unsaid_length`와 해석 문장 교체를 넣기 전과 후. 경로는 저장소 기준으로 고쳐 적었다.
