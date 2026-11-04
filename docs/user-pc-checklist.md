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
- [ ] 팩 폴더마다 백엔드 계열이 하나뿐인가 (`engines\b12066\cpu\`에 `ggml-vulkan.dll`/`ggml-cuda.dll` 등이 섞여 있지 않은가 — 섞이면 다른 백엔드가 그래프를 가로채 `Compute error`가 난다)
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

## P2 (이번 단계)

- [ ] `node scripts/fetch-pdfium.mjs` 로 `%LOCALAPPDATA%\DocuLens\pdfium\pdfium.dll` 이 설치되고(SHA-256 핀 일치),
      설정 > 정보 또는 라이브러리 상단에 "PDFium 없음" 경고가 사라지는가 (`--bundle` 은 설치파일 동봉용)
- [ ] 실제 스캔 PDF(100쪽 이상)를 드롭 → 미리보기(신규/중복/미지원 사유) → 가져오기 작업 진행률 → 라이브러리 썸네일 → 뷰어 탐색이 매끄러운가 (200 DPI 기준 쪽당 ≈1초 목표)
- [ ] 디지털 PDF 의 텍스트 레이어가 뷰어 "텍스트" 탭에 보이는가 (`has_text_layer`)
- [ ] 한컴오피스로 만든 **실제 HWPX** / Word **DOCX** 를 넣으면 "변환본" 배지와 함께 페이지가 보이는가
      (WebView2 `PrintToPdf` 경로 — A4 여백 18/16 mm, 표 병합 셀, 그림, 글상자 텍스트). 수식·다단·글상자 위치는 근사임
- [ ] **HWP 5.0** 파일(일반/배포용)이 열리는가, 암호화 HWP 는 "지원하지 않습니다" 로 거부되는가
- [ ] 휴대폰 JPG(EXIF Orientation 6/8)가 올바른 방향으로 렌더되는가
- [ ] HEIC 파일은 미리보기에서 "JPG 로 변환 후" 사유로 제외되는가
- [ ] inbox 감시: 탐색기에서 `inbox\` 에 파일을 복사하면 3초 안정화 후 자동 등록되는가, 큰 파일 복사 중에는 "복사 중"으로 대기하는가
- [ ] 탐색기에서 문서를 다른 폴더로 옮기면 "다시 검사" 후 파일 없음 → 다시 가져오기 시 "다시 연결"로 복구되는가
- [ ] 뷰어 회전 저장 후 페이지 이미지가 바뀌고 다시 렌더에도 유지되는가
- [ ] 삭제가 Windows 휴지통으로 가는가; USB 드라이브에서는 `.doculens\trash\` 로 가는가
- [ ] OneDrive/네트워크 공유 폴더를 워크스페이스로 열면 `workspace.db` 가 `journal_mode=DELETE` 로 동작하는가 (로컬은 WAL)
- [ ] 두 번째 창 없이 변환 중 숨은 인쇄 창이 작업 표시줄에 나타나지 않는가 (`skip_taskbar`)

## P3 이후 (예정)

- [ ] 골든셋 정확도(분류·OCR CER·KIE 필드 F1) — 사용자 제공 골든셋
- [ ] NSIS 설치파일(Lite/Full) 오프라인 설치
- [ ] 부록 B 1~17 전체 (설계 문서 참조)
