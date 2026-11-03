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

## P1 이후 (예정)

- [ ] llama-server CUDA 팩(cu121/cu129) 기동, `--list-devices` free VRAM 값
- [ ] Qwen3.5-4B Q6_K + mmproj 로드 시 실측 VRAM (설계 §2.4 표와 비교)
- [ ] 골든셋 정확도(분류·OCR CER·KIE 필드 F1) — 사용자 제공 골든셋
- [ ] NSIS 설치파일(Lite/Full) 오프라인 설치
- [ ] 부록 B 1~17 전체 (설계 문서 참조)
