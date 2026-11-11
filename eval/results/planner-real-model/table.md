| 종류 | 항목 수 | 0.7.0 | 0.8.0 |
|---|---:|---:|---:|
| 대화 | 2 | 0/2 (0 %) | 2/2 (100 %) |
| 질문 | 2 | 0/2 (0 %) | 2/2 (100 %) |
| 명령 | 1 | 0/1 (0 %) | 1/1 (100 %) |
| 설정 | 4 | 0/4 (0 %) | 4/4 (100 %) |
| **전체** | 9 | **0/9 (0 %)** | **9/9 (100 %)** |
| 확인 없이 실행됐을 잘못된 변경 | | 5건 | 0건 |
| 응답 시간 중앙값 | | 117.3 s | 3.9 s |

| id | 문장 | 0.7.0 | 0.8.0 |
|---|---|---|---|
| ko-94 | UI 배율 120% | ✕ set_profile {"profile": "balanced"} · 확인 없이 실행 | ○ set_setting ui.scale_percent=120 |
| ko-96 | 배율을 130 으로 변경해줘 | ✕ set_theme {"theme": "high-contrast"} · 확인 없이 실행 | ○ set_setting ui.scale_percent=130 |
| ko-97 | DPI 를 200 으로 변경해줘 | ✕ set_theme {"theme": "high-contrast"} · 확인 없이 실행 | ○ set_setting processing.render_dpi=200 |
| ko-104 | 글자가 작아서 잘 안 보여, 화면 배율을 120으로 올려줘 | ✕ set_theme {"theme": "system"} · 확인 없이 실행 | ○ set_setting ui.scale_percent=120 |
| ko-110 | 안녕하세요. | ✕ noop | ○ answer mode=chat |
| ko-111 | 점심 뭐 먹을지 고민이네 | ✕ clarify | ○ answer mode=chat |
| ko-112 | 애자일 방법론이 뭐야? | ✕ answer | ○ answer mode=general |
| ko-115 | 프로젝트 별 내용을 요약해줘 | ✕ summarize {"target": "each_doc"} · 확인 없이 실행 | ○ summarize {"group_by": "project"} |
| ko-119 | 지난달 견적서 합계를 메일로 보내줘 | ✕ aggregate · 확인 카드 | ○ aggregate · 확인 카드 |
