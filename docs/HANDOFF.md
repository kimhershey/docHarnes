# Handoff — DocuLens implementation status

> 단계마다 새 세션으로 진행합니다. 다음 세션은 이 파일과 `CLAUDE.md`,
> `docs/design/doculens-design-v1.1.md` 만 읽으면 이어서 작업할 수 있습니다.
> 사용자 PC에서 설치·실행·빌드·검증하는 절차는 `docs/RUN-GUIDE.md`, 확인 항목은 `docs/user-pc-checklist.md`.

## 범위 결정 (2026-11-03)

- **1차(v1.0)**: P0 ~ P4 + 플래너 핵심 op (import, classify, ocr, extract,
  validate, review, search, answer, summarize, organize/rename/move, export,
  설정 변경/open/help 등). 토큰 예산 ≈ 400~550만.
- **2차(v1.1)**: 문서 자동 생성(§3.13), 프로젝트 정합성·보완요청서(§3.15),
  그 외 §4.0 "다음" 항목.
- 작업 방식: 단계별 새 세션 + 데모 체크포인트, GitHub 커밋/푸시, 빌드 로그
  축약·모듈 단위 컴파일 검증, 리뷰 서브에이전트는 단계 말에만.
- 설치파일 빌드(Actions "Windows installer (manual)")는 **요청된 작업이 모두 끝나고 전체 검증(Rust 테스트·clippy·vitest·
  Playwright·푸시한 커밋의 CI)에 FAILED가 없음을 확인한 뒤에만** 실행한다(사용자 결정 2026-11-09). 작업 도중 빌드 금지.
- **버전별 브랜치**(사용자 결정 2026-11-10 "앞으로는 마이너와 패치 버전을 올려서 브랜치를 생성하고 작업하자"): 작업을 시작할 때 버전을 올리고
  (새 기능·구조 변경 = 마이너, 수정 = 패치; `Cargo.toml`과 **`Cargo.lock`**(`cargo update -w --offline` 또는 아무 cargo 빌드 — 고지 잡은
  `cargo metadata --locked`로 돈다)·`package.json`/lock·`src-tauri/tauri.conf.json`·모의 백엔드 버전 문자열·`npm run notices`)
  `main`에서 `release/<major.minor.patch>` 브랜치를 만들어 그 위에서 작업·WIP 푸시·설치파일 빌드를 한다(`v0.5.0` 같은 이름은 릴리스 태그와 겹치므로
  쓰지 않음). **버전 범프 커밋은 그 브랜치의 첫 병합 가능 상태**이므로 `[skip ci]` 없이 푸시해 CI를 확인한다(0.6.0의 범프 e6a2e4f는 `Cargo.lock`·SBOM이
  옛 버전인 채 CI를 건너뛰었고, 사용자가 PR #20으로 병합해 `main`의 고지/SBOM 잡이 깨졌다 — 통합 커밋 2efc181 이후 다시 병합하면 해소).
  끝나면 사용자가 PR로 `main`에 병합하고 게시 실행이 `v<버전>` 태그를 만든다. 첫 적용: `release/0.5.0`(`main` 4c2a7ca = PR #18까지 병합된
  `phase/p6-p7`에서 분기; `phase/*` 브랜치는 0.1.0~0.4.0과 0.5.0 초반의 기록으로만 남김).

## 현재 상태 (2026-11-23)

- `main` = ceb9830: 1.0.3까지 병합됨(PR #34 · #35 · #36). 최신 Release는 **v1.0.4**(아래 "1.0.4" 절, 2026-11-23 게시).
- `release/1.0.4`는 아직 `main`에 병합되지 않았다(병합은 사용자가 PR로 한다).
- P0 ~ P7과 0.2.0 ~ 1.0.2는 모두 `main`에 들어 있다. 그 판들의 기록(이 파일의 지난 절, `docs/plan/` 계획서, `docs/audit/` 점검 목록, `docs/releases/`의 1.0.2 이하 릴리스 노트,
  `eval/results/planner-real-model/`의 1.0.2 이하 측정 결과)은 **1.0.4에서 지웠다**(사용자 지시 2026-11-23 "다 지워줘"). 필요하면 git에서 본다:
  `git show acd551a:docs/HANDOFF.md`, `git show acd551a:docs/plan/v1.0.0.md`, `git show acd551a:docs/audit/v0.7.0.md` 등(acd551a = 지우기 직전 커밋).
- 지금의 코드 구조는 `CLAUDE.md`의 "Layout" 절이 기준이다. 사용자 PC 확인 항목은 `docs/user-pc-checklist.md`(판별 절이 그대로 있다).

## 판별 기록

### 1.0.3 — Release v1.0.3 게시: 요약은 결과 하나 — 문서별 요약을 없애고 범위의 문서 전체를 하나의 결과로 요약 (2026-11-22 요청 · 같은 날 게시; 브랜치 `release/1.0.3`, `release/1.0.2` e757eda에서 분기; 릴리스 노트 `docs/releases/v1.0.3.md`)

- **사용자 요청(2026-11-22)**: "파일 별 요약이라는 동작은 누가 만든거야? 파일별 요약은 없애고 모든 파일의 내용을 하나의 결과로 요약하는거로 변경해줘"
- **누가 만들었나**: 사용자가 요청한 동작이 아니다. 처음 설계안 부록 D의 `summarize | target ∈ {each_doc, set}`(P0 스캐폴드 84143a7, 2026-11-03)와 P5 구현(57e7819, 2026-11-04)에서 `each_doc`(문서마다 요약 하나)이 기본값으로 들어갔다.
  `target = set`은 0.8.0에서야 실제로 구현됐고(그 전에는 두 값이 같은 문서별 반복), 1.0.2는 "AA 프로젝트 요약해줘"에 문서별 요약 + 전체 요약 하나를 냈다. 사용자에게 그대로 답했다.
- **한 일**:
  - `knowledge::answer::summarize_documents(deps, req, documents, on_delta, on_step)` — 문서 여러 건의 청크를 준 순서대로(문서 안에서는 청크 순) `summarize_rows`에 넘긴다: 근거 예산에 들어가면 호출 한 번, 넘치면 부분 정리 뒤 합치기(한 부분에 여러 문서의 대목이 섞일 수 있다).
    근거는 요약이 인용한 대목만 남긴다(`cited_sources`; 번호 유지 — 카드는 `index`로 찾는다. 인용이 없으면 문서마다 첫 대목). 본문 없는 문서는 빼고 id를 돌려준다. 문서가 하나면 `summarize`와 같은 길. 답변 기록에는 `document_id` 없이 한 건.
    `summarize_rows_stepwise`의 `on_step(만든 호출, 예상 호출)`으로 진행을 알린다.
  - `exec/summarize.rs::run` — 문서별 반복과 `combine`(문서 요약문을 다시 요약) · `CombineRoom` · `batches`를 없앴다. `group_documents`는 기준이 없으면 묶음 하나(`all` — "전체", 문서가 하나면 그 이름), 묶음마다 `summarize_documents` 한 번.
    payload `{summaries: [], groups, group_by, skipped?, failed?}`(`summaries`는 빈 목록으로 남긴다 — 화면이 요약 단계의 결과를 알아보는 키). 요약 줄 "문서 N건의 내용을 하나로 요약"(계획 미리보기도 같음).
    묶음별 요약에서 실패한 묶음의 문서는 `failed`(다른 묶음의 요약에 들어간 문서는 제외), 묶음이 하나뿐이면 그 실패가 단계의 실패다. `answer.rs`의 `summary_reply_tokens` · `text_room` · `text_cost` · `clip_to_tokens`는 쓰는 곳이 없어 지웠다.
  - 계획: `Op::Summarize.target`은 읽기 전용 옛 필드(대화 카드 · 대기 작업에 저장된 계획이 계속 읽히도록 남김; 검증기가 지우고 아무도 쓰지 않는다). `schema.rs` 작업 표 · 문법 · 프롬프트 예시 · `opSpecs.ts` · `types.ts`에서 뺐다.
    `rules.rs`: 요약 규칙은 `target` 없이("하나로 · 합쳐 · 묶어"를 보던 분기 삭제). `guard.rs`: `summary_about` · `where_to_look`의 `target = set` 대입 삭제.
  - "문서별로 요약해줘": `guard::per_document`("문서별 · 파일별 · 문서마다 · 각 문서 · 각각 · 하나씩 · 따로따로 · each document · one by one", 다른 묶음 기준을 말하지 않았을 때) → `keep_the_split`이 요약의 묶음을 지우고
    `split_check`가 `grouping_dropped`("문서별 요약은 만들지 않습니다. 범위의 문서를 모두 읽어 하나의 결과로 요약합니다", `unmet` = "문서마다 따로 요약하는 부분")로 확인을 받는다. 규칙 계획에도 적용된다.
  - 화면: 결과 카드는 묶음(지금은 "전체 · 문서 N건" 하나)을 그리고, 1.0.2 이하에서 만든 결과의 문서별 항목은 그대로 보인다(`result-summary-document`). 계획 편집 화면의 "대상"과 i18n `each_doc` · `set` 삭제.
    모의 백엔드: `mock-knowledge.ts::summarizeDocuments`, `mock-planner.ts`(실행 · 규칙 · 검증 · `perDocumentSplit` · 미리보기). **모의 백엔드의 `ruleMerge`를 코어와 맞췄다** — 따옴표 밖 낱말만 보고, "요약 · 정리 · 집계 · 비교"가 있으면 병합이 아니다
    (브라우저 모드에서 "선택한 문서를 하나로 묶어 요약해줘"가 PDF 병합으로 읽혔다; 코어는 원래 요약이었다).
  - 평가: 골든셋 196 → 201건(요약 기대값 16건의 `target`은 `null` = 없어야 함; 새 문장 ko-150~ko-153 · en-37). 기준선 192/201 = 95.5 %(틀린 9건은 1.0.2와 같다).
  - 진단: `tests/real_summary.rs`에 `DOCULENS_SUMMARY_TOGETHER=1`(폴더의 문서 전체를 하나로 요약 — 호출 수, 인용한 문서 출력).
- **실제 모델(Gemma 4 E4B Q4_K_M, CPU 4코어)로 돌려 보고 더 고친 것**:
  - 요약 결과물 — 첫 실행은 "**개발용역계약서.txt 요약**" · "**정보보안지침.txt 요약**" 제목 아래 문서마다 따로 정리했고(한 카드에 담긴 문서별 요약), 서로 다른 주제를 다룰 뿐인 두 문서로 "상충:" 단락을 썼다.
    `documents_instruction`을 고쳤다: 문서별로 나누지 말고(문서 이름을 제목으로 달지 않는다) 전체를 주제별로 묶어 쓴다, 같은 사항을 다르게 적었을 때만 문서를 밝힌다, 다른 주제는 상충이 아니다.
    고친 뒤 두 번: 한 목록 14항목, 인용 14곳(두 문서), 호출 1번, 332 ~ 342초(첫 실행 440초 — 다른 빌드와 겹쳤다). 14항목 중 짧은 문서(글의 23 %)는 2항목이다 — "고루 담으라"는 문구를 넣어도 같았다.
  - 계획 12문장(`one-summary-1.0.3.golden.json`) — 9/12 → 12/12. 모델이 "전체 · 모든 · all"이 든 세 문장에 `length: long`을 붙였다 → `guard::unsaid_length`(검증 전): 요약의 길이는 문장(주제 밖의 말)이 말한 대로, 말하지 않았으면 없음.
    `rules::length_said`에 "간략 · 한 문장 · 구체적 · in detail …"을 더했다. 문서별 요약 문장의 해석 문장("전체 문서의 내용을 문서별로 요약합니다.")은 `split_check`가 "범위의 문서를 모두 읽어 하나로 요약합니다"로 바꾼다.
    결과 파일 `gemma4-e4b-q4km-1.0.3-one-summary{-before-guards,}.json`, README "1.0.3" 절.
- **검증**: 로컬 전체 검증(마지막 트리) — `cargo fmt --check`, clippy(1.100), `cargo test --workspace`(코어 566 통과 · 8 ignored), `npm run typecheck`, vitest 362, Playwright 144, 골든 게이트 192/201 = 95.5 %, 실제 모델용 12문장 규칙 경로 12/12.
  CI: 버전 커밋 6cb98ef — run 45004530346 성공(7개 잡). 코드 커밋 2c4761b — run 45013923659 성공(7개 잡). 문서 커밋 f1af00b(릴리스 노트 · 이 절; `[skip ci]` 없이 올림) — run 45014523709 성공(7개 잡).
- **확인하지 못한 것**: 사용자 PC(Windows · 설치파일), 문서가 많아 여러 부분으로 나눠 읽는 범위의 실제 모델 실행(자동 테스트의 모의 모델과 1.0.1의 긴 문서 측정뿐), 묶음별 요약의 결과물, E2B · 12B · GPU, 영어 문서.
- **이 판에서 고치지 않은 것**: 주제 요약을 묶음별로 나누기, 잠긴 문서 안내 · 옛 `.ppt` 형식 판별(위 1.0.2 절), PDF 그림 속 글자(1.0.1 절), UX 검토 153건. 요약 하나의 길이는 문서 수가 아니라 근거 토큰 예산을 따른다(문서가 많으면 문서당 분량이 준다).
- **릴리스(2026-11-22; 사용자 지시 "release")**: 설치파일 run **45042618023**(`--ref release/1.0.3`, `ref=f1af00b…`, `variant=lite`, `publish=true`) 성공 → **Release v1.0.3** 게시(태그 `v1.0.3` = f1af00b, 2026-11-22T16:29:08Z, 본문 = `docs/releases/v1.0.3.md`).
  `DocuLens-Setup-Lite-1.0.3-x64.exe` 52,031,968 B, sha256 `4d62d19357df400ab8b0a65542682e76ca2a3b5624ff313d7b9c70a67af08ea5`(게시된 `.sha256` · 자산 다이제스트와 일치) + `.sha256` · `THIRD-PARTY-NOTICES.md` · `sbom.cdx.json`.
  Full 설치파일은 만들지 않았다(1.0.3은 Lite만). 태그 뒤의 커밋(이 기록)은 문서뿐이다.
- **다음**: 병합은 사용자가 한다 — PR kimhershey/docHarnes#34(1.0.1 → `main`)가 먼저, 그다음 1.0.2, 1.0.3(각 브랜치는 앞 판에서 갈라졌다).
  사용자 PC 확인은 체크리스트 "1.0.3" 절 — 요약이 하나로 나오는지, 문서별 구획 없이 통합됐는지, 짧은 문서의 내용이 빠지지 않는지, 문서가 많은 프로젝트에서 걸리는 시간.

### 1.0.4 — 코드 정리 · 옛것 지우기 · 간단한 오류 점검 (2026-11-23; 브랜치 `release/1.0.4`, `main` ceb9830에서 분기) — Release v1.0.4 게시

- **사용자 요청(2026-11-23)**: ① "코드정리와 함께 오류검출 간단하게 수행해줘. 옛날것들은 다 지워버려도 괜찮아. GIT 에 남아있으니까" ② (저장된 데이터를 읽는 데 필요해 남겨 둔 것의 목록을 보고) "다 지워줘".
- **1차(acd551a) — 어디에서도 부르지 않던 코드**: 웹의 쓰이지 않는 내보내기 27개, 화면이 부르지 않던 명령 9개(`prerouter_resolve_date`, `documents_tags`, `documents_set_page_text`, `pipeline_schema_get`,
  `pipeline_form_derive`, `pipeline_document_schema`, `pipeline_revalidate`, `pipeline_table_csv`, `pipeline_table_export` — 래퍼 · 셸 명령 · 등록 · 모의 처리기까지), 코어의 쓰이지 않는 `pub fn` 16개와 상수 3개.
  모의 백엔드의 `documents_set_page_text`만 남겼다(테스트가 쪽 글을 바꾸는 데 쓴다).
- **찾아서 고친 오류**: 오류 코드 17개에 문구가 없어 사용자에게 영어 코드가 그대로 보였다(`merge_needs_2_to_20_documents`, `boundaries_required`, `no_tables`, `folder_outside_workspace`, `topic_empty` 등). `errors.reasons`에 한국어 · 영어 문구를 넣었다.
- **점검했고 문제가 없던 것**: 웹이 부르는 명령이 모두 셸에 등록돼 있음, 코드가 쓰는 i18n 키가 모두 있음, `console.log` · `@ts-ignore` · TODO 없음, 셸의 `expect` 7곳은 모두 깨질 수 없는 불변식.
- **2차("다 지워줘") — 옛 데이터를 읽으려고 남겨 두었던 것**:
  - **요약의 `target` 칸**: `Op::Summarize`는 `length` · `group_by` · `topic`뿐이다(`SummarizeTarget` 삭제). `"target"`이 든 계획은 읽히지 않는다.
  - **설정 전용 동작 5개**(`set_theme` · `set_language` · `set_profile` · `set_model` · `set_inbox_watch`)와 `Op::canonical`: 동작은 58개(`OpKind::ALL`), 설정 변경은 `set_setting{key, value}` 하나다.
    만드는 쪽은 `Op::setting` · `Op::set_theme` · `Op::set_language` · `Op::set_profile` · `Op::set_model` · `Op::set_inbox_watch`(모두 `set_setting`을 만든다). 프리라우터의 `PreRoutedOp::SetTheme` · `SetLanguage`(슬래시 명령의 결과 타입)는 그대로다.
  - **설정의 옛 별칭**(`render_dpi` · `review_threshold` · `pii_masking` 등 0.6.0 이전의 짧은 이름)과 `ExplainScope`의 `collection` 별칭. `journal_days`는 별칭이 아니라 제 키라서 남았다.
  - **숨겨 둔 옛 양식 6종**(세금계산서 · 계산서 · 영수증 · 사업자등록증 · 계약서 · 신청서): 내장 양식은 소프트웨어 개발 문서 13종뿐이고 `Builtin.hidden` · `repo::enable_legacy_builtins`가 없다.
    여섯 양식의 정의는 테스트 자료로만 남았다(`schema::business_samples` · `schema::sample` · `repo::add_business_samples`, 모두 `#[cfg(test)]` — 금액 · 사업자번호 · 합계 규칙 검사에 쓴다).
    이미 그 행이 들어 있는 워크스페이스는 행이 DB에 그대로 남아 보통의 저장된 양식처럼 동작한다(새로 만든 워크스페이스에는 생기지 않는다). 양식 화면의 "꺼 둔 기본 양식" 접힘 줄은 사용자가 끈 내장 양식이 있을 때만 보인다.
    유형 이름표(`projects/repo.rs`의 유형 표, `doc_type_aliases`, `forms.rs`의 이름→유형 표)의 세금계산서 · 영수증 등은 사용자가 등록하는 양식이 쓰므로 남겼다.
  - **옛 문서 템플릿 4종**(견적서(품목) `quotation` · 거래명세서 `transaction_statement` · 계약서 `contract` · 회의록 `minutes`): 각각 소프트웨어 문서 쪽 짝(`estimate` · `dev_contract` · `meeting_minutes`)이 있고 어디에서도 고를 수 없었다.
    **`report`(보고서)와 `official_letter`(공문)는 남겼다** — 숨긴 6종으로 적어 두었지만 실제로는 쓰이고 있었다: `report`는 양식을 말하지 않은 초안의 기본 틀(`draft::resolve_spec`의 마지막 줄)이고,
    `official_letter`는 "공문 형식으로"처럼 말하면 닿는다(`rules.rs::TEMPLATE_WORDS`, `match_keywords_any`). 둘은 `TemplateKind::hidden`(목록에 안 보임)으로 그대로다. 지울지는 사용자 결정(지우면 기본 틀을 무엇으로 할지 정해야 한다).
  - **옛 프로젝트 템플릿 3종**(인증 심사 · 거래처 등록 · 구매 정산): `repo::business_templates`(`#[cfg(test)]`)로 테스트 자료가 됐다.
  - **모의 백엔드**도 같게: 설정 전용 동작 · 별칭 · 옛 양식 6종 · 옛 템플릿 4종 삭제, i18n의 해당 키 삭제.
  - **문서**: `docs/plan/`(7개), `docs/audit/`(2개), `docs/releases/`의 v0.2.0 ~ v1.0.2(12개), 이 파일의 P0 ~ 1.0.2 절, `eval/results/planner-real-model/`의 1.0.2 이하 결과 · 문장 세트 21개. 체크리스트 · 실행 가이드 · 설계 문서에서 지운 문서를 가리키던 문구를 뺐다.
- **이 판으로 올리면 읽히지 않게 되는 것**(사용자가 알고 정함): 1.0.2 이하에서 저장된 대화 카드 · 대기 작업 중 `target`이 든 요약 계획이나 설정 전용 동작이 든 계획(그 카드는 다시 실행할 수 없다),
  옛 템플릿 4종으로 만든 저장 초안(`generate/store.rs::list`가 읽지 못하는 파일을 건너뛰어 목록에서 빠진다 — 파일은 `.doculens/drafts/`에 남는다), 옛 별칭으로 적힌 설정 변경 계획.
  읽히지 않는 대기 작업의 명세는 `jobs/mod.rs`가 `None`으로 읽으므로 작업 목록은 깨지지 않는다. 화면은 모르는 동작 이름을 그대로 글자로 보여 준다(`describePlan.ts::opLabel`의 `defaultValue`). 다시 실행을 눌렀을 때의 오류 문구는 확인하지 못했다.
- **검증**(2차까지 넣은 상태, 2026-11-23): `cargo fmt --check`, clippy(1.100, `-D warnings`), `cargo test --workspace`(코어 566 · PDFium 포함), 골든 게이트 192/201 = 95.5 %(기준선과 같음) · 1.0.3 문장 세트 12/12,
  `npm run typecheck`, vitest 362, Playwright 144 — 모두 통과. 실제 모델로는 다시 재지 않았다(모델에게 보이는 동작 목록 · 문법은 1.0.3과 같다 — 설정 전용 동작 5개는 1.0.0부터 모델에게 보이지 않았다).
- **릴리스(2026-11-23; 사용자 지시 "릴리즈 실행")**: 설치파일 run **45139584096**(`--ref release/1.0.4`, `ref=cb18ebb3bb9fc2a5e7d9c07e7b9dbc942c6a1bd1`, `variant=lite`, `publish=true`) 성공 — **Release v1.0.4**(게시 2026-11-23T08:54:16Z, 태그 `v1.0.4` = cb18ebb, 그 커밋의 CI 7개 잡 통과: run 45135458341 · 45135463607).
  `DocuLens-Setup-Lite-1.0.4-x64.exe` 51,977,426 B, sha256 `463f0d8e7208582153a422452470a89ad4d66e3874bc66b867ac4201ef0db003`(게시된 `.sha256` 파일과 자산 다이제스트가 같음), `THIRD-PARTY-NOTICES.md`, `sbom.cdx.json`. 본문은 `docs/releases/v1.0.4.md`.
  Full 설치파일은 만들지 않았다(Lite만). 태그 뒤의 커밋(이 기록)은 문서뿐이다.
- **남긴 것**: Qwen 계열 드롭인 모델 지원(사용자가 넣은 모델 파일을 읽는 현재 기능), `RulesFirst` 해석 모드(평가 게이트가 쓴다), DB 마이그레이션 0001 ~ 0012(기존 워크스페이스를 여는 데 필요).

## 다음 단계 메모 (v1.x)

- **1.0.3 다음(2026-11-22)**: 위 "1.0.3" 절(요약은 결과 하나 · Release v1.0.3). 다음 판 후보: 문서가 많은 범위의 요약 시간 줄이기(문서별 중간 요약을 저장해 다시 쓰기 — 지금은 매번 전체 본문을 읽는다), 한 문서에 치우치지 않게 문서마다 몫을 정하는 방식이 필요한지 사용자 확인, 묶음별 요약 결과물의 실제 모델 확인.
- **1.0.2 다음(2026-11-22)**: 1.0.2 판(그 절은 지움 · git 기록; 두 칸 결정 반영 · Release v1.0.2). 다음 판 후보: "…에 대해" 없이 쓴 "AA 프로젝트의 BB 내용 요약해줘"를 모델 해석에 맡기지 않고 읽기, 주제 요약을 묶음별로 나누기("프로젝트별로 BB에 대해 요약"), 잠긴 문서 안내(오류 사유를 우리말로, 범위에서 빠진 문서를 알리기, 암호 입력은 별도 결정), LibreOffice가 저장한 옛 `.ppt`의 형식 판별, 주제 요약에서 문서 전체를 읽는 방식(검색에 걸리지 않은 대목까지)이 필요한지 사용자 확인.
- **1.0.1 다음(2026-11-17)**: 1.0.1 판(그 절은 지움 · git 기록) — Release v1.0.1 게시 완료. 그 절의 "구현하면서 정한 것" 세 가지는 사용자에게 알렸고 이의 없이 게시를 지시받았다(다르게 원하면 바꾼다). 다음 판 후보: PDF 그림 속 글자가 읽히지 않는 경우들(진단 목록이 그 절에 있다 — 사용자 결정 "아직 고치지 않기"), UX 검토 153건, 어시스턴트의 "현재 프로젝트" 기본 범위를 모의 백엔드에도 적용하기(브라우저 모드는 문장이 프로젝트를 말해야 그 범위가 된다).
- **0.7.0 점검 목록에서 열려 있는 것**(점검 목록 문서는 1.0.4에서 지움): PKG-1(CUDA 재배포 자산이 앱 EULA 없이 게시돼 있다 — 결정: 앞으로 Lite만 게시, 이미 게시된 Full · CUDA 자산은 그대로),
  PLN-20(오래 걸리는 작업이 확인 없이 시작된다 — 사용자 결정으로 변경 없음), 부분 수정으로 남은 KNO-14(답변 중지가 첫 글자 전 · 대기 중에는 듣지 않는다), PRJ-4(구분 전환이 문서의 전역 역할을 바꾼다 — 데이터 모델 유지),
  PKG-11(앱 데이터 위치가 둘이라 로그 경로 · 보존 · 제거가 어긋난다), PKG-14(빌드 잡의 토큰 권한이 넓고 액션이 태그로만 고정).

1. 남은 부록 D op(GUI 전용: `render`·`rotate`·`split`·`merge`·`add_to_kb`·`manage_collection`·`define_term`·`set_tier`·
   `set_backend`·`import_model`·`download_model`·`benchmark`·`open_workspace`)을 플래너에서 UI 액션으로 연결.
2. P4·P5 숙제 중 미착수: bbox 그라운딩(디지털 PDF 문자 박스), 배치 PDF 자동 경계 탐지, 컬렉션 폴더 감시, OCR 출력 건전성
   검사, 스키마 화면 라우트 이동 차단(데이터 라우터 전환), 실모델 플래너 토큰 수·지연 보정.
3. P7 후속: 실모델 골든셋 CI(self-hosted GPU), 앱 EULA(부록 E, Full 설치파일 전제), Full 설치파일 축소(실측 1.1 GB / 설치 3.1 GB;
   cu121 `-Xfatbin=-compress-all`, 팩의 도구 DLL·실행파일 제거, CUDA 팩 해시의 매니페스트 고정), Release 게시(`publish=true`),
   NSIS 설치 경로와 앱데이터 폴더 겹침 확인.

## 알려진 제약

- HF 차단으로 `models/registry.json`의 크기·SHA-256 미고정(`verified: false`); 다운로드 검증은
  핀이 생기면 자동 적용. `engines/manifest.json`의 cpu·vulkan 자산 해시는 첫 설치파일 빌드 로그로 고정(2026-11-08), `linux`만 미고정.
- GPU 없음: CUDA 팩은 CI에서 빌드했지만 실행 검증(`--list-devices`, `CUDA0` 로그)·Vulkan 팩·실모델 VRAM 실측은 사용자 PC
  (`docs/user-pc-checklist.md` P1 항목).
- Linux 빌드는 CI 전용(배포 범위 외). Windows 실창 E2E(tauri-driver)는 사용자 PC.
- 모의 백엔드의 프리라우터는 Rust 구현의 부분 집합(브라우저 데모 용도).
- 라우터의 `memory` 실측은 CPU 전용 빌드에서는 비어 있음(GPU 장치가 있을 때만 채워짐).
- WebView2 `PrintToPdf`(HWPX/DOCX/HWP 변환본)는 이 환경에서 실행 불가 — Linux WebKitGTK 경로로 동일 파이프라인을
  검증했고 Windows 경로는 컴파일·clippy 검사(교차 타깃 + CI)만 거침(`docs/user-pc-checklist.md` P2 항목). 숨은 창에서
  `PrintToPdf`가 완료되는지, 실패한 내비게이션(오류 페이지)이 인쇄되지 않는지는 사용자 PC에서 확인.
- 이 환경에는 한글 글꼴이 제한적이라 변환본 렌더 품질(글꼴 대체)은 사용자 PC에서 확인.
- 실제 bge-m3·Qwen3.5 모델을 받을 수 없어 임베딩·답변 생성은 해시 임베더·대본 모델(테스트)과 모의 라우터로만 검증했다
  (경로·스트리밍·인용 파싱). 실제 검색 품질(재현율·인용 정확도)과 답변 품질은 사용자 PC 골든셋 항목
  (`docs/user-pc-checklist.md` P3).
- lindera.dev 차단으로 ko-dic 원본은 bitbucket에서 받는다(sha256 핀). CI는 빌드 결과를 캐시한다.
