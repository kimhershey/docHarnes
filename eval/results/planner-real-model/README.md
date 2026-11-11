# 플래너 — 실제 모델 측정 (0.7.0 대 0.8.0)

같은 문장을 0.7.0(`main` dbb598e)과 0.8.0(`release/0.8.0`)의 `doculens-eval planner --engine …`로 돌린 결과입니다(점검 목록 PLN-21).

| | |
|---|---|
| 모델 | Gemma 4 E4B Q4_K_M (`gemma4-e4b-q4km`, 레지스트리의 SHA-256과 일치하는 파일) |
| 엔진 | llama-server, CPU 4코어, 컨텍스트 8,192 |
| 문장 | 골든셋(`eval/golden/planner.json`, 163문장)의 일부 — 0.7.0 10문장, 0.8.0 9문장. 아래 표는 두 판 모두 돌린 문장만 |
| 채점 | 두 결과 파일을 0.8.0의 기준(`scripts/rescore-planner.py`)으로 다시 채점. 0.7.0 실행파일은 확인 대기·반영 못 한 부분·종류를 모르므로 자체 정답 표시는 쓰지 않음 |
| 0.8.0 실행파일 | 병합 커밋 3f51704(계약 구현 8a0a2d5 + 검토 반영 2969117 포함)에서 빌드. 그 뒤의 수정 2fcd1b5(되묻기 답으로 설정 완성, 사용자가 쓴 말만 근거로 삼기, 프로젝트별 결과에 열린 프로젝트 범위를 덮지 않기)는 단위 테스트로 확인했고 이 측정에는 들어 있지 않음 |

**한계**: 골든셋 전체가 아닙니다. CPU에서 플랜 한 건에 수 분이 걸려(다른 빌드·테스트와 CPU를 나눠 쓴 상태) 사용자 PC 보고 문장과 그 주변 문장부터 돌렸습니다.
응답 시간은 그런 조건에서 잰 값이라 참고용입니다(0.8.0의 설정·인사는 모델을 부르지 않아 1 ms 미만, 대화·일반 질문은 계열 호출 한 번).
전체 측정은 GPU가 있는 PC에서 `eval/README.md`의 절차로 합니다.

## 결과

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

## 파일

- `gemma4-e4b-q4km-0.7.0.json`, `gemma4-e4b-q4km-0.8.0.json` — 배치별 결과를 합친 것(문장별 플랜, 출처, 응답 시간).
- `table.md` — 위 표(`python3 scripts/rescore-planner.py eval/golden/planner.json 0.7.0=… 0.8.0=… --ids … --md table.md`).
