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
