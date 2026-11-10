# DocuLens (가칭) — 오프라인 VLM 문서처리 데스크톱 클라이언트

Windows PC 안에서만 동작하는 문서처리 앱입니다. 로컬 비전-언어 모델
(Gemma 4 E2B/E4B/12B, llama.cpp; 임베딩 EmbeddingGemma 2 — 0.4.0부터 중국계 모델 제외)로 제안요청서·견적서·개발 계약서·요구사항 정의서 등 소프트웨어 개발 문서를 분류·OCR·추출하고,
사전에 적재한 지식 베이스(RAG)를 근거로 질의·요약·문서 작성을 돕습니다.
모든 처리는 로컬에서 이루어지며 네트워크로 아무것도 보내지 않습니다.

- 설계 문서: [`docs/design/doculens-design-v1.1.md`](docs/design/doculens-design-v1.1.md)
- 진행 상황·다음 단계: [`docs/HANDOFF.md`](docs/HANDOFF.md)
- 라이선스 정책(소스 공개 의무 없는 라이선스만): [`docs/LICENSE-POLICY.md`](docs/LICENSE-POLICY.md)
- 실행 가이드(설치·실행·빌드·검증·문제 해결): [`docs/RUN-GUIDE.md`](docs/RUN-GUIDE.md)
- 사용자 PC 검증 체크리스트: [`docs/user-pc-checklist.md`](docs/user-pc-checklist.md)

## 기술 스택

Tauri 2 · Rust 코어 · React 19 / TypeScript / Vite · Tailwind CSS v4 ·
Radix UI · i18next(한국어 기본, 영어) · SQLite(rusqlite, FTS5, sqlite-vec) · llama-server 사이드카(라우터 모드) ·
PDFium · lindera ko-dic

## 개발

```bash
npm install
npm run typecheck && npm test && npm run build
cargo test -p doculens-core
```

데스크톱 창 실행(Windows, 또는 webkit2gtk가 설치된 Linux):

```bash
npm run tauri dev
```

브라우저 미리보기(모의 백엔드, Tauri 불필요):

```bash
npm run dev
```

UI 스모크 테스트(Playwright, 브라우저 모드):

```bash
npm run e2e
```

## 현재 단계

P0~P5(v1.0)에 이어 P6·P7(v1.1)까지 구현: 엔진·모델 관리(드롭인·가져오기·다운로드·핫스왑), 워크스페이스·문서
(가져오기·HWP/HWPX/DOCX/XLSX/PPTX/텍스트 변환·렌더·뷰어·끌어다 놓기), 참고 자료(0.5.0: 프로젝트 = 업무 문서 + 참고 자료, 공통 참고 자료; 청킹·하이브리드 검색·근거 기반 답변),
파이프라인(분류·OCR·추출·검증·검토 큐·양식 등록), 자연어 플래너(확인 카드·실행기, op 43종), 영속 작업 러너,
정리/이름변경/되돌리기, 집계, 내보내기 5종, **문서 생성(템플릿·등록 양식 채우기·HWPX/DOCX/PDF/MD)**,
**프로젝트 정합성 검사·보완요청서**, 평가 하네스와 양자화 스코어카드, NSIS Lite/Full 릴리스 워크플로,
THIRD-PARTY-NOTICES/SBOM 자동화, 보존/파기 일일 작업, 완료 알림·절전 방지.
자세한 현황은 `docs/HANDOFF.md`, 설치·실행은 `docs/RUN-GUIDE.md`.
