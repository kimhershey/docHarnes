# 플래너 — 실제 모델 측정 (0.7.0 대 0.8.0)

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

## 파일

- `gemma4-e4b-q4km-0.7.0.json`, `gemma4-e4b-q4km-0.8.0.json` — 배치별 결과를 합친 것(문장별 플랜, 출처, 응답 시간; 다시 잰 문장의 첫 결과는 `first_measurement`).
- `table.md` — 위 표(`python3 scripts/rescore-planner.py eval/golden/planner.json 0.7.0=… 0.8.0=… --ids … --md table.md`).
