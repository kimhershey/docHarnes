# 플래너 — 실제 모델 측정 (0.7.0 대 0.8.0, 그리고 1.0.0)

같은 문장을 0.7.0(`main` dbb598e)과 0.8.0(`release/0.8.0`)의 `doculens-eval planner --engine …`로 돌린 결과입니다(점검 목록 PLN-21).

| | |
|---|---|
| 모델 | Gemma 4 E4B Q4_K_M (`gemma4-e4b-q4km`, 레지스트리의 SHA-256과 일치하는 파일) |
| 엔진 | llama-server, CPU 4코어, 컨텍스트 8,192 |
| 문장 | 골든셋(`eval/golden/planner.json`, 163문장)의 일부 — 0.7.0 23문장, 0.8.0 26문장. 아래 표는 두 판 모두 돌린 23문장 |
| 채점 | 두 결과 파일을 0.8.0의 기준(`scripts/rescore-planner.py`)으로 다시 채점. 0.7.0 실행파일은 확인 대기·반영 못 한 부분·종류를 모르므로 자체 정답 표시는 쓰지 않음 |
| 0.8.0 실행파일 | 두 가지. **A** = 병합 커밋 3f51704(계약 구현 8a0a2d5 + 검토 반영 2969117), **B** = 642f20c(A 뒤의 수정 + 아래 "다시 잰 문장"의 검사 세 가지). 12문장은 A로, 11문장은 B로 측정 |

## 결과

| 종류 | 항목 수 | 0.7.0 | 0.8.0 |
|---|---:|---:|---:|
| 대화 | 2 | 0/2 (0 %) | 2/2 (100 %) |
| 질문 | 11 | 3/11 (27 %) | 9/11 (82 %) |
| 명령 | 2 | 1/2 (50 %) | 2/2 (100 %) |
| 설정 | 8 | 0/8 (0 %) | 7/8 (88 %) |
| **전체** | 23 | **4/23 (17 %)** | **20/23 (87 %)** |
| 확인 없이 실행됐을 잘못된 변경 | | 11건 | 0건 |
| 응답 시간 중앙값 | | 169.4 s | 160.7 s |

| id | 문장 | 0.7.0 | 0.8.0 |
|---|---|---|---|
| ko-05 | 이 계약서의 계약기간과 위약금 조항 알려줘 | ○ answer | ○ answer |
| ko-07 | 다크 테마로 바꾸고 모델을 12B로 올려 | ✕ set_theme,set_profile {"profile": "balanced"} · 확인 없이 실행 | ○ set_theme,set_model {"id": "gemma4-12b-q4km"} |
| ko-94 | UI 배율 120% | ✕ set_profile {"profile": "balanced"} · 확인 없이 실행 | ○ set_setting ui.scale_percent=120 |
| ko-96 | 배율을 130 으로 변경해줘 | ✕ set_theme {"theme": "high-contrast"} · 확인 없이 실행 | ○ set_setting ui.scale_percent=130 |
| ko-97 | DPI 를 200 으로 변경해줘 | ✕ set_theme {"theme": "high-contrast"} · 확인 없이 실행 | ○ set_setting processing.render_dpi=200 |
| ko-104 | 글자가 작아서 잘 안 보여, 화면 배율을 120으로 올려줘 | ✕ set_theme {"theme": "system"} · 확인 없이 실행 | ○ set_setting ui.scale_percent=120 |
| ko-105 | 배율 130으로 바꾸고 견적서 요약해줘 | ✕ set_theme,summarize {"length": "short", "target": "each_doc"} · 확인 없이 실행 | ○ set_setting,summarize {"target": "each_doc"} |
| ko-106 | UI 배율이 뭐야? | ✕ clarify | ○ answer mode=general |
| ko-107 | 다크 테마 문서 찾아줘 | ○ search | ✕ clarify |
| ko-108 | 이 문서 해상도 300으로 다시 렌더해줘 | ○ rerender | ○ rerender |
| ko-109 | DPI 200으로 바꿨어 | ✕ set_theme {"theme": "high-contrast"} · 확인 없이 실행 | ✕ set_setting processing.render_dpi=200 · 확인 카드 |
| ko-110 | 안녕하세요. | ✕ noop | ○ answer mode=chat |
| ko-111 | 점심 뭐 먹을지 고민이네 | ✕ clarify | ○ answer mode=chat |
| ko-112 | 애자일 방법론이 뭐야? | ✕ answer | ○ answer mode=general |
| ko-113 | 대한민국 수도가 어디야? | ✕ answer | ○ answer mode=general |
| en-30 | what does SLA stand for? | ✕ answer | ○ answer mode=general |
| ko-114 | 이 계약서에서 지체상금이 얼마로 정해져 있어? | ○ answer | ○ answer |
| ko-115 | 프로젝트 별 내용을 요약해줘 | ✕ summarize {"target": "each_doc"} · 확인 없이 실행 | ○ summarize {"group_by": "project"} |
| ko-116 | 유형별로 문서를 요약해줘 | ✕ summarize {"target": "each_doc"} · 확인 없이 실행 | ○ summarize {"group_by": "doc_type"} |
| en-31 | summarize the documents by project | ✕ summarize {"target": "each_doc"} · 확인 없이 실행 | ○ summarize {"group_by": "project"} |
| ko-119 | 지난달 견적서 합계를 메일로 보내줘 | ✕ aggregate · 확인 카드 | ○ aggregate · 확인 카드 |
| ko-120 | 견적서를 담당자별로 요약해줘 | ✕ summarize {"target": "each_doc"} · 확인 없이 실행 | ○ summarize {} · 확인 카드 |
| ko-121 | 다음 문장을 다듬어줘: 금번 과업은 당사가 수행하며 기한은 12월말까지로 하… | ✕ draft_document · 확인 카드 | ✕ draft_document · 확인 카드 |

## 읽는 법과 한계

- **골든셋 전체가 아닙니다.** CPU에서 플랜 한 건에 수 분이 걸려(다른 빌드·테스트와 CPU를 나눠 쓴 상태) 사용자 PC에서 보고된 문장과 그 주변 문장부터 돌렸습니다. 전체 측정은 GPU가 있는 PC에서 `eval/README.md`의 절차로 합니다.
- **응답 시간은 참고용도 못 됩니다.** 다른 작업과 CPU를 나눠 쓴 상태의 값입니다. 구조상의 차이만 말할 수 있습니다: 0.8.0의 설정 한 문장과 인사는 모델을 부르지 않고(1 ms 미만), 대화·일반 질문은 계열 호출 한 번으로 끝납니다(이 측정에서 약 4초).
- **0.8.0이 틀린 3문장**
  - `ko-107` "다크 테마 문서 찾아줘": 검색 대신 무엇으로 찾을지 되물었습니다(0.7.0은 검색). 잘못 실행된 것은 없지만 이 문장은 0.7.0보다 나빠졌습니다.
  - `ko-109` "DPI 200으로 바꿨어"(알림): 모델은 여전히 설정 변경으로 읽습니다. 다만 실행하지 않고 확인을 기다립니다(기대값은 대화 답변).
  - `ko-121` 붙여 넣은 문장 다듬기: 모델이 문서 초안 작성으로 읽었습니다(확인 카드). 이 측정 뒤에 그런 플랜을 글 다듬기로 되돌리는 검사를 넣었고(모델이 낸 플랜을 재생하는 단위 테스트로 확인), 실제 모델로 다시 재지는 않았습니다.
- 0.8.0만 돌린 3문장(설계 예시 `ko-01` ○ · `ko-02` ✕ · `ko-03` ○)은 표에 없습니다. `ko-02`는 결과는 같지만(추출 뒤 세 열만 엑셀로) 필드 선택을 내보내기 단계에 적어 기대값과 다르게 채점됐습니다.

## 다시 잰 문장

실행파일 A로 잰 뒤 틀린 문장을 보고 검사 세 가지(문장에 없는 묶음 기준, 질문·알림 문장의 설정 변경, 문장이 말한 문서 유형을 대상에 넣기)를 넣고 B로 다시 쟀습니다. 위 표에는 다시 잰 결과가 들어 있습니다.

| id | 문장 | A (첫 측정) | B (다시 측정) |
|---|---|---|---|
| ko-120 | 견적서를 담당자별로 요약해줘 | summarize{group_by: folder} · 확인 없이 실행 | summarize · 확인 카드 · 범위 {"doc_type": "estimate"} |
| ko-105 | 배율 130으로 바꾸고 견적서 요약해줘 | set_setting{key: ui.scale_percent, value: 130}, summarize · 확인 없이 실행 | set_setting{key: ui.scale_percent, value: 130}, summarize · 범위 {"doc_type": "estimate"} |
| ko-109 | DPI 200으로 바꿨어 | set_setting{key: processing.render_dpi, value: 200} · 확인 없이 실행 | set_setting{key: processing.render_dpi, value: 200} · 확인 카드 |

## 1.0.0 — 모델에게 보이는 동작을 줄인 뒤 (계획 4절)

2026-11-16, 같은 모델(Gemma 4 E4B Q4_K_M; 이번에는 비전 프로젝터도 함께 로드) · 같은 엔진(llama-server, CPU, 컨텍스트 8,192)으로 쟀습니다. 골든셋 전체가 아니라 4절이 건드린 문장과 사용자 문장입니다.
응답 시간은 다른 빌드 · 테스트와 CPU를 나눠 쓴 값이라 적지 않습니다(계획 한 건에 수 초 ~ 6분).

### 4절이 건드린 20문장 — 19/20

설정 전용 동작 5개(`set_theme` · `set_language` · `set_profile` · `set_model` · `set_inbox_watch`)와 `explain`을 모델의 동작 목록에서 뺀 뒤(커밋 0764b2a), 그 동작이 기대값이던 문장과 주변 문장을 돌렸습니다.
확인 없이 실행됐을 잘못된 변경 0건.

| id | 문장 | 결과 |
|---|---|---|
| ko-05 | 이 계약서의 계약기간과 위약금 조항 알려줘 | ○ answer · 범위 {"doc_ids": ["d1a2"]} |
| ko-07 | 다크 테마로 바꾸고 모델을 12B로 올려 | ○ set_setting ui.theme="dark", set_setting models.active_model="gemma4-12b-q4km" |
| ko-14 | inbox 감시 꺼줘 | ○ set_setting processing.inbox_watch=false · 모델 호출 없음 |
| ko-53 | 모델을 E4B로 바꿔 | ○ set_setting models.active_model="gemma4-e4b-q5km" |
| ko-54 | 정확도 우선 프로파일로 바꿔줘 | ○ set_setting models.profile="accuracy" |
| ko-55 | 이 조항이 무슨 뜻인지 설명해줘 | ○ answer {"knowledge": false, "mode": "general"} |
| ko-56 | 고대비 테마 켜줘 | ○ set_setting ui.theme="high-contrast" |
| ko-57 | UI를 영어로 바꿔줘 | ○ set_setting ui.language="en" · 확인 카드 |
| en-14 | switch to the light theme | ○ set_setting ui.theme="light" · 모델 호출 없음 |
| mx-06 | theme dark로 바꿔줘 | ○ set_setting ui.theme="dark" · 모델 호출 없음 |
| mx-07 | model을 12B로 switch | ○ set_setting models.active_model="gemma4-12b-q4km" |
| ko-98 | 프로파일 정확도 | ○ set_setting models.profile="accuracy" · 모델 호출 없음 |
| ko-104 | 글자가 작아서 잘 안 보여, 화면 배율을 120으로 올려줘 | ○ set_setting ui.scale_percent=120 |
| ko-105 | 배율 130으로 바꾸고 견적서 요약해줘 | ○ set_setting ui.scale_percent=130, summarize {"target": "each_doc"} · 범위 {"filter": {"doc_type": "estimate"}} |
| ko-106 | UI 배율이 뭐야? | ○ answer {"mode": "general"} |
| ko-108 | 이 문서 해상도 300으로 다시 렌더해줘 | ○ rerender {"dpi": 300} · 범위 {"doc_ids": ["d1a2"]} |
| ko-109 | DPI 200으로 바꿨어 | ✕ set_setting processing.render_dpi=200 · 확인 카드 |
| ko-111 | 점심 뭐 먹을지 고민이네 | ○ answer {"knowledge": false, "mode": "chat"} |
| ko-112 | 애자일 방법론이 뭐야? | ○ answer {"mode": "general"} |
| ko-115 | 프로젝트 별 내용을 요약해줘 | ○ summarize {"group_by": "project"} |

- 설정 문장 14건 가운데 13건이 `set_setting` 한 가지 모양으로 나왔습니다. 0.8.0에서도 잰 9문장(`ko-07` · `ko-104` · `ko-105` · `ko-106` · `ko-108` · `ko-109` · `ko-111` · `ko-112` · `ko-115`)은 정답 여부가 같습니다(둘 다 8/9; 설정 변경의 모양만 `set_setting`으로 바뀜).
- `ko-109` "DPI 200으로 바꿨어"(알림)는 0.8.0과 같습니다: 모델은 설정 변경으로 읽고, 실행하지 않고 확인을 기다립니다.
- `ko-57` "UI를 영어로 바꿔줘"는 맞게 읽었지만 확인 카드가 뜹니다. 문장에 설정 이름("언어")이 없어서입니다(설정 이름과 값이 둘 다 문장에 있어야 확인 없이 실행 — 0.8.0의 규칙 그대로).
- `ko-55` "이 조항이 무슨 뜻인지 설명해줘": 모델이 **문서를 찾지 않는 답변**(`knowledge: false` · `mode: general`, 범위 없음)으로 적었습니다. 채점 기준(`answer`도 정답)으로는 맞지만 열린 문서의 조항을 묻는 말이므로,
  이 측정 뒤에 `guard::document_question`을 열린 문서 · 선택한 문서를 가리키는 질문("이 조항" · "이 문서" · "this clause")에도 적용했습니다 — 답변을 문서 근거로 되돌리고 범위를 열린 문서로 채웁니다
  (이 모델 출력을 재생하는 단위 테스트 `a_question_that_points_at_the_open_document_is_answered_from_it`; 다시 잰 결과는 아래 8문장 표).

### 사용자 문장 5개 (0.8.2의 `user-sentences-0.8.2.golden.json`) — 2/5 → 5/5

`explain`을 숨기기만 한 상태(0764b2a)에서는 "…무엇인지 설명해줘 / 알려줘" 세 문장이 문서를 찾지 않는 답변으로 빠졌습니다. 프로젝트 이름이 든 질문을 문서 근거 답변으로 되돌리는 검사
`guard::document_question`을 넣고 다시 쟀습니다.

| id | 문장 | 검사 전 | 검사 후 |
|---|---|---|---|
| u-1 | AAA 프로젝트 내에서 AAA 가 무엇인지 요약해서 설명해줘 | ✕ answer {"knowledge": false} | ○ answer · 범위 {"filter": {"project_id": "p-aaa"}} |
| u-2 | AAA 프로젝트 내에서 AAA 가 무엇인지 요약해서 설명해줘(되묻기에 "유형 상관없이 전부"라고 답한 경우) | ○ summarize {"target": "set"} · 범위 {"filter": {"project_id": "p-aaa"}} | ○ summarize {"target": "set"} · 범위 {"filter": {"project_id": "p-aaa"}} |
| u-3 | AAA 프로젝트에서 AAA가 무엇인지 검색해줘 | ○ search {"mode": "hybrid", "query": "AAA", "top_k": 10} · 범위 {"filter": {"project_id": "p-aaa"}} | ○ search {"mode": "hybrid", "query": "AAA", "top_k": 10} · 범위 {"filter": {"project_id": "p-aaa"}} |
| u-4 | AAA 프로젝트에서 AAA가 무엇인지 알려줘 | ✕ answer {"knowledge": false, "mode": "general"} | ○ answer · 범위 {"filter": {"project_id": "p-aaa"}} |
| u-5 | AAA가 무엇인지 설명해줘 | ✕ answer {"knowledge": false, "mode": "general"} | ○ answer · 범위 {"filter": {"project_id": "p-aaa"}} |

### 열린 문서를 가리키는 질문 · 원본 고치기 문장 8개 — 6/8 → 8/8

위 `ko-55`의 검사를 넓힌 뒤, 그리고 `edit_text`에 `original`(원본 파일을 고쳐 새 판으로 저장 — 계획 2-1절)을 더한 뒤 쟀습니다. 첫 측정에서 글자 바꾸기 두 문장이 대상을 잘못 잡았습니다:
모델이 `ko-75`에는 범위를 비워 두었고(문서가 열려 있는데도 앱이 "어떤 문서를 대상으로 할까요?"라고 되물음), `ko-130`에는 따옴표 안의 "11월 30일"을 범위의 날짜 조건으로 적었습니다(그날의 문서를 찾게 됨).
검사 `guard::edit_target`을 넣고 두 문장을 다시 쟀습니다 — 따옴표 안의 글은 바꿀 글이지 조건이 아니고, 대상을 말하지 않은 글자 바꾸기는 선택한 문서, 없으면 열린 문서에 합니다(규칙 경로와 같음). 실행 전에는 항상 확인 카드가 뜹니다.

| id | 문장 | 첫 측정 | 검사 후 |
|---|---|---|---|
| ko-05 | 이 계약서의 계약기간과 위약금 조항 알려줘 | ○ answer · 범위 {"doc_ids": ["d1a2"]} | (다시 재지 않음) |
| ko-55 | 이 조항이 무슨 뜻인지 설명해줘 | ○ answer · 범위 {"doc_ids": ["d1a2"]} | (다시 재지 않음) |
| ko-75 | '㈜한빛'을 '(주)한빛'으로 바꿔줘 | ✕ clarify | ○ edit_text {"find": "㈜한빛", "replace": "(주)한빛"} · 확인 카드 · 범위 {"doc_ids": ["d1a2"]} |
| ko-106 | UI 배율이 뭐야? | ○ answer {"mode": "general"} | (다시 재지 않음) |
| ko-112 | 애자일 방법론이 뭐야? | ○ answer {"mode": "general"} | (다시 재지 않음) |
| ko-130 | 원본에서 '11월 30일'을 '12월 31일'로 바꿔줘 | ✕ edit_text {"find": "11월 30일", "original": true, "replace": "12월 31일"} · 확인 카드 · 범위 {"filter": {"date_from": "2026-11-30", "date_to": "2026-11-30"}} — 따옴표 안의 날짜가 조건이 됨 | ○ edit_text {"find": "11월 30일", "original": true, "replace": "12월 31일"} · 확인 카드 · 범위 {"doc_ids": ["d1a2"]} |
| ko-131 | 이 문서 원본 파일의 '㈜한빛'을 '(주)한빛'으로 고쳐서 새 판으로 저장해줘 | ○ edit_text {"find": "㈜한빛", "original": true, "replace": "(주)한빛"} · 확인 카드 · 범위 {"doc_ids": ["d1a2"]} | (다시 재지 않음) |
| en-34 | replace 'Nov 30' with 'Dec 31' in the origi… | ○ edit_text {"find": "Nov 30", "original": true, "replace": "Dec 31"} · 확인 카드 · 범위 {"doc_ids": ["d1a2"]} | (다시 재지 않음) |

- `ko-55`는 이제 열린 문서(`d1a2`)에서 찾아 답하는 질문으로 나옵니다. 일반 지식 질문(`ko-106` · `ko-112`)은 문서가 열려 있어도 그대로입니다.
- 원본 고치기 세 문장(`ko-130` · `ko-131` · `en-34`)은 모두 `original: true`와 확인 카드로 나왔고, "원본"을 말하지 않은 `ko-75`에는 `original`이 없습니다.
- 첫 측정의 채점은 대상을 보지 않아 `ko-130`이 정답으로 찍혔습니다(결과 파일에는 7/8). 그 뒤 네 문장의 기대값에 대상(열린 문서)을 넣었고, 위 표의 첫 측정은 그 기준으로 적었습니다.

### "참고 자료로 지정" 문장 4개 — 3/4 → 4/4

사용자 결정(2026-11-16, 계획 4-3): "이 문서를 참고 자료로 지정해줘"는 문서에 표시만 붙이지 않고 "프로젝트에 넣기" 화면을 참고 자료 쪽으로 엽니다(`set_role`의 `knowledge`가 그 일을 합니다 —
실행기가 화면 동작을 넘기고, 바뀌는 것은 대화상자에서 확인할 때입니다). 동작 설명을 그렇게 고친 뒤 네 문장을 쟀습니다.

| id | 문장 | 첫 측정 | 검사 후 |
|---|---|---|---|
| ko-76 | 이 문서를 참고 자료로 표시 | ○ set_role {"role": "knowledge"} | ○ (같음) |
| ko-132 | 이 문서를 참고 자료로 지정해줘 | ○ set_role {"role": "knowledge"} | ○ (같음) |
| ko-133 | 선택한 문서들 참고자료로 지정 | ✕ manage_reference {"action": "add", "project": "한빛 납품"} · 확인 카드 — 문장에 없는 프로젝트를 모델이 골랐음 | ○ set_role {"role": "knowledge"} |
| en-35 | mark this document as reference material | ○ set_role {"role": "knowledge"} | ○ (같음) |

- `ko-133`에서 모델은 문장이 말하지 않은 프로젝트("한빛 납품" — 측정 컨텍스트에 있는 프로젝트)의 참고 자료에 넣겠다고 적었습니다. 확인 카드가 뜨므로 바로 실행되지는 않지만, 확인을 누르면 사용자가 고르지 않은
  프로젝트에 들어갑니다. 검사 `guard::reference_place`를 넣었습니다 — 문장이 표시만 하는 말("…로 지정 / 표시")이고 프로젝트 · 묶음 · 공통 자료를 말하지 않았는데 계획이 어딘가에 넣는 것이면
  화면을 여는 단계로 바꿉니다. 문장이 프로젝트나 묶음을 말했으면 그대로 둡니다.
- 네 문장 모두 확인 카드 없이 나옵니다(화면을 여는 단계는 아무것도 바꾸지 않으므로). 계획의 요약 문장은 누가 썼든 "문서를 프로젝트의 참고 자료로 넣는 화면을 엽니다"로 맞춥니다.
- 응답 시간은 문장당 22 ~ 125초였습니다(CPU). 첫 문장과 영어 문장(지시문이 영어라 4,040토큰)이 약 2분, 같은 한국어 지시문(약 2,960토큰)을 이어 쓴 두 문장이 22초입니다.
- 네 문장만 잰 것입니다. "참고 자료로 넣어줘" · 프로젝트 이름을 말한 문장 · E2B · GPU는 재지 않았습니다.

## 파일

- `gemma4-e4b-q4km-0.7.0.json`, `gemma4-e4b-q4km-0.8.0.json` — 배치별 결과를 합친 것(문장별 플랜, 출처, 응답 시간; 다시 잰 문장의 첫 결과는 `first_measurement`).
- `table.md` — 위 표(`python3 scripts/rescore-planner.py eval/golden/planner.json 0.7.0=… 0.8.0=… --ids … --md table.md`).
- `gemma4-e4b-q4km-0.8.2-user-sentences.json` — 0.8.2: 사용자 PC에서 보고된 문장("AAA 프로젝트 내에서 AAA 가 무엇인지 요약해서 설명해줘")과 그 변형 5개를 같은 모델·엔진으로 돌린 결과(5/5, 되묻기·검증 오류 없음).
  문장과 컨텍스트는 `user-sentences-0.8.2.golden.json` / `user-sentences-0.8.2.context.json`. 모델이 쓴 `explain`을 답변으로 바꾸는 검사(`guard::explain_what_was_said`)는 이 측정 뒤에 들어갔다(결과 파일의 `operations`는 그 전 모습).
- `gemma4-e4b-q4km-1.0.0-section4.json` — 1.0.0 4절: 골든셋 20문장(`--ids ko-05,ko-07,ko-14,ko-53,ko-54,ko-55,ko-56,ko-57,en-14,mx-06,mx-07,ko-98,ko-104,ko-105,ko-106,ko-108,ko-109,ko-111,ko-112,ko-115`).
- `gemma4-e4b-q4km-1.0.0-user-sentences-before-guard.json`, `gemma4-e4b-q4km-1.0.0-user-sentences.json` — 1.0.0 4절: 사용자 문장 5개, `guard::document_question`을 넣기 전과 후.
- `gemma4-e4b-q4km-1.0.0-edit-original.json`, `gemma4-e4b-q4km-1.0.0-edit-original-remeasured.json` — 1.0.0 2-1절: 위 8문장(`--ids ko-55,ko-05,ko-106,ko-112,ko-75,ko-130,ko-131,en-34`)과, `guard::edit_target`을 넣은 뒤의 두 문장.
- `gemma4-e4b-q4km-1.0.0-reference-material-before-guard.json`, `gemma4-e4b-q4km-1.0.0-reference-material.json` — 1.0.0 결정 4-3: 위 4문장(`--ids ko-76,ko-132,ko-133,en-35`), `guard::reference_place`를 넣기 전과 후.
  결과 파일의 경로(`golden` · `context` · `engine.binary`)는 저장소 기준으로 고쳐 적었다.
