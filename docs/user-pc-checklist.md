# 사용자 PC 검증 체크리스트 (부록 B 요약)

이 클라우드 세션은 GPU가 없고 huggingface.co 가 차단되어 있어 아래 항목은
사용자 PC(Windows 10/11 + NVIDIA 8 GB)에서 확인해야 합니다. 결과는 이슈로
남기거나 다음 세션에 전달해 주세요.

## P0 (이번 단계)

- [ ] `npm run tauri dev` 로 창이 뜨고 온보딩 → 홈이 동작하는가 (WebView2 Runtime 필요)
- [ ] 테마 4종(라이트/다크/시스템/고대비) 전환 시 **창 제목표시줄**도 같이 바뀌는가
- [ ] Windows 설정 > 접근성 > 대비 테마를 켜면 앱이 고대비 팔레트를 따르는가 (`forced-colors`)
- [ ] 한국어/영어 전환이 즉시 반영되고 재시작 후 유지되는가
- [ ] `%LOCALAPPDATA%\DocuLens\settings.json` 이 생성되고, `active_workspace` 를 직접 수정하면 다음 시작 시 반영되는가
- [ ] 워크스페이스 폴더에 `.doculens\{workspace.json, lock, models\, cache\, …}`, `inbox\`, `documents\`, `exports\` 가 생성되는가
- [ ] 같은 워크스페이스를 두 번째 인스턴스로 열면 잠금 안내가 뜨는가 (강제 열기 포함)
- [ ] 모델 관리자 > 하드웨어 감지에 GPU 이름 / VRAM / 드라이버 CUDA 버전 / CC 가 표시되는가 (NVML)
- [ ] 산정 티어가 가용 VRAM 기준 표와 일치하는가 (8 GB 카드 → `8 GB`, 권장 Q6_K/Q5_K_M/Q4_K_M)
- [ ] "필요 파일 목록" 텍스트의 Hugging Face URL 이 실제로 존재하는가 → 존재하면 크기·SHA-256 을 `models/registry.json` 에 고정하고 `verified: true` 로 변경
- [ ] 한글 IME 로 명령 입력 중 Enter 가 조합 도중에 전송되지 않는가 (어시스턴트, 명령 바, 팔레트)
- [ ] UI 배율 90~150% 가 Windows DPI(125%, 150%)와 독립적으로 동작하는가

## P1 (이번 단계)

- [ ] `node scripts/fetch-engine.mjs --tag b12066 --packs cpu,vulkan --pin` 으로 공식 팩을 받고
      `engines/manifest.json` 에 SHA-256 이 기록되는가 (자산 이름 패턴 `bin-win-cpu-x64` / `bin-win-vulkan-x64` 확인)
- [ ] 모델 관리자 > 엔진 카드에 실행 파일·버전(b12066)·백엔드(cu121/cu129/vulkan/cpu)·설치된 팩이 표시되는가
- [ ] "장치 조회"가 `CUDA0: … (8188 MiB, 7126 MiB free)` 를 보여 주고, 하드웨어 패널 새로고침 후 티어 근거가 "엔진 보고 가용 VRAM 기준"으로 바뀌는가
- [ ] 실제 Qwen3.5-4B GGUF + mmproj 를 `.doculens\models\<폴더>\` 에 복사하면 2초 안에 "준비됨"으로 감지되는가 (복사 중에는 "복사 중")
- [ ] 같은 파일을 GGUF 가져오기(복사/이동/하드링크)로 넣어도 동일하게 등록되는가, LICENSE 가 같이 복사되는가
- [ ] "해시 검증" 후 레지스트리 항목이 "검증됨"으로 바뀌는가 (해시 핀이 없으면 이름 일치 유지) → 얻은 SHA-256 을 `models/registry.json` 에 고정
- [ ] 엔진 자동 시작 → 활성 모델 자동 로드 → 상태바 "엔진: 준비 · VRAM 사용 x / 8 GiB" 가 설계 §2.4 표와 비슷한가 (Q6_K 32K ≈ 6.2 GiB)
- [ ] 모델 테스트 다이얼로그의 한국어 응답과 tok/s (RTX 4060 기대 50~80 tok/s)
- [ ] 두 모델(Q6_K ↔ Q4_K_M) 로드/언로드 핫스왑, 상주 2개 설정 시 메모리 마진 동작
- [ ] 작업 관리자에서 llama-server 를 강제 종료하면 "재기동 중 (1회)" → 복구 → 모델 재로드되는가; 3회 연속 종료 시 "중단됨" 안내
- [ ] 앱을 비정상 종료(작업 관리자)해도 llama-server 자식 프로세스가 남지 않는가 (Job Object)
- [ ] 온라인 다운로드(허용 시)가 이어받기·SHA-256 검증과 함께 동작하는가
- [ ] 엔진 로그에 `--reasoning off` 관련 경고/오류가 없고 `<think>` 가 응답에 없는가
- [ ] 백엔드를 Vulkan 으로 강제했을 때 이미지 요청이 TDR 없이 완료되는가 (image-max-tokens 1440)

## P2 이후 (예정)

- [ ] 골든셋 정확도(분류·OCR CER·KIE 필드 F1) — 사용자 제공 골든셋
- [ ] NSIS 설치파일(Lite/Full) 오프라인 설치
- [ ] 부록 B 1~17 전체 (설계 문서 참조)
