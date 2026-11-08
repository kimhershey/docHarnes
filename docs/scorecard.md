# 양자화 스코어카드 (사용자 PC 측정 템플릿)

> 설계 §3.11 "양자화별(Q4/Q5/Q6, mmproj F16/BF16) 스코어카드 유지". 아래 표는 **빈 템플릿**입니다.
> 측정은 사용자 PC에서 `doculens-eval pipeline` / `doculens-eval planner` 로 모델·양자화마다 `results.json` 을 만든 뒤
> `cargo run -p doculens-eval -- scorecard --inputs <results.json>... --out docs/scorecard.md` 로 **같은 열 구성의 표를 생성**해
> 이 파일을 덮어씁니다(절차·지표 정의: `eval/README.md`). 골든셋은 사용자 제공(§3.11, 마스킹된 실제 문서 유형별 ≥50쪽).

| 모델 | 양자화 | 엔진 | 장치 | 분류 F1 | 분류 정확도 | 필드 정확일치 | ANLS* | OCR CER | 페이지 p50 | 페이지 p95 | VRAM 피크 | 플래너 정확도 |
|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Qwen3.5-4B | Q4_K_M + mmproj F16 | cu121 b_____ | RTX 4060 (8188 MiB) | | | | | | | | | |
| Qwen3.5-4B | Q5_K_M + mmproj F16 | cu121 b_____ | RTX 4060 (8188 MiB) | | | | | | | | | |
| Qwen3.5-4B | Q6_K + mmproj F16 | cu121 b_____ | RTX 4060 (8188 MiB) | | | | | | | | | |
| Qwen3.5-4B | Q4_K_M + mmproj BF16 | cu121 b_____ | RTX 4060 (8188 MiB) | | | | | | | | | |
| Qwen3.5-4B | Q4_K_M + mmproj F16 | vulkan b_____ | RTX 4060 (8188 MiB) | | | | | | | | | |
| Qwen3.5-9B (12 GB 참조) | Q4_K_M + mmproj F16 | cu121 b_____ | | | | | | | | | | |
| Qwen3.5-2B (CPU 참조) | Q4_K_M + mmproj F16 | cpu b_____ | CPU | | | | | | | | | |

## 측정 조건(함께 기록)

- 골든셋: 이름 / 버전 / 문서 수 / 유형별 쪽수 — `results.json.manifest`, `document_count`
- 프롬프트 버전: 파이프라인 `p4.1`, 플래너 `p5.1` (`crates/doculens-core/src/pipeline/mod.rs`, `planner/mod.rs`)
- llama.cpp 빌드·백엔드: `results.json.engine.build_number`, `engine.backend`
- 드라이버 / CUDA 버전, 컨텍스트 크기(`engine.ctx`), 프로파일(`engine.profile`), 정밀 모드 여부(`precise`)
- 측정 시각: `generated_at`

## 해석 기준

- **회귀 게이트**: 기준 구성(현재 기본 모델·양자화) 대비 어느 지표든 0.5 pt 초과 하락이면 교체·배포 차단(§3.11). 플래너는
  `doculens-eval planner --baseline` 이 자동 판정하고, 파이프라인 지표는 이 표에서 비교합니다.
- **양자화 선택**: 분류 F1·필드 정확일치·ANLS* 가 같은 수준이면 VRAM 피크와 p95 가 낮은 쪽을 택합니다(8 GB 카드에서 1.5~3 GiB 여유 유지 — 설계 §2.6).
- **OCR CER** 은 낮을수록 좋고, 나머지 비율 지표는 높을수록 좋습니다.
