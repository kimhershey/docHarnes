// Extra names a document type goes by in commands and rules ("계약서" → dev_contract), shared by the
// mock planner, the mock pipeline's name classifier and the mock project rules. Mirror of the core's
// `pipeline::schema::doc_type_aliases`.
export const DOC_TYPE_ALIASES: Record<string, string[]> = {
  rfp: ["제안요청서", "제안 요청서", "rfp", "request for proposal"],
  proposal: ["제안서", "proposal"],
  sow: ["과업지시서", "과업 지시서", "과업내용서", "sow", "statement of work"],
  srs: ["요구사항정의서", "요구사항 정의서", "요구사항명세서", "요구사항", "srs", "requirements"],
  dev_contract: ["개발계약서", "개발 계약서", "용역계약서", "계약서", "계약", "contract", "agreement"],
  wbs: ["wbs", "일정표", "작업분류체계", "schedule"],
  progress_report: ["진척보고서", "진척 보고서", "주간보고서", "주간 보고", "월간보고서", "월간 보고", "진도보고", "progress report", "weekly report"],
  meeting_minutes: ["회의록", "회의 록", "minutes", "meeting minutes"],
  test_report: ["테스트결과서", "테스트 결과서", "시험결과서", "테스트 보고서", "test report", "test results"],
  change_request: ["변경요청서", "변경 요청서", "변경요청", "change request", "cr"],
  acceptance: ["검수확인서", "검수 확인서", "검수조서", "완료보고서", "검수", "acceptance", "acceptance certificate"],
  estimate: ["견적서", "견적", "estimate", "quotation", "quote"],
  tax_invoice: ["세금계산서", "세금 계산서", "tax invoice", "invoice"],
  invoice_exempt: ["계산서", "면세계산서", "exempt invoice"],
  business_registration: ["사업자등록증", "사업자 등록증", "등록증", "business registration", "registration certificate"],
  contract: ["계약서", "계약", "contract", "agreement"],
  application_form: ["신청서", "신청", "application", "application form"],
  quotation: ["견적서", "견적", "quotation", "quote", "estimate"],
  transaction_statement: ["거래명세서", "거래명세표", "명세서", "transaction statement", "statement"],
  receipt: ["영수증", "receipt"],
  financial_statement: ["재무제표", "financial statement", "financials"],
  purchase_order: ["발주서", "purchase order"],
};

export function docTypeAliases(slug: string): string[] {
  return DOC_TYPE_ALIASES[slug] ?? [];
}
