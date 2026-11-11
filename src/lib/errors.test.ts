// `describeError` against the real strings and the error shapes the shell really sends (PIP-3): a stable
// reason code is translated whichever error class carries it — as `code`, as the reason of an `invalid`
// error, behind a detail or wrapped by the DB layer.
import { describe, expect, it } from "vitest";

import en from "@/i18n/en.json";
import ko from "@/i18n/ko.json";
import i18n, { initI18n } from "@/i18n";

import { describeError, errorReasonKey } from "./errors";

initI18n("ko");
const t = i18n.t.bind(i18n);
const reasons = ko.errors.reasons;

describe("describeError", () => {
  it("translates the typed command errors whose code is the reason (`typed_error` / `retype` in commands/pipeline.rs)", () => {
    for (const code of ["schema_doc_type_invalid", "schema_name_empty", "schema_not_found", "schema_builtin", "no_active_model", "no_model", "description_empty", "field_value_empty", "no_documents"] as const) {
      expect(describeError({ code, message: code }, t), code).toBe(reasons[code]);
    }
  });

  it("translates a reason wrapped by the DB layer (repo `typed()` → rusqlite → `CoreError::Db`)", () => {
    expect(describeError({ code: "db", message: "database error: Invalid parameter name: schema_name_duplicate" }, t)).toBe(reasons.schema_name_duplicate);
    expect(describeError({ code: "db", message: "database error: Invalid parameter name: project_not_found" }, t)).toBe(reasons.project_not_found);
    expect(describeError({ code: "db", message: "database error: Invalid parameter name: template_builtin" }, t)).toBe(reasons.template_builtin);
  });

  it("translates the reason of an `invalid` error, with or without the core's prefix or a trailing detail", () => {
    expect(describeError({ code: "invalid", message: "invalid value: no_findings" }, t)).toBe(reasons.no_findings);
    expect(describeError({ code: "invalid", message: "invalid value: export_over_document" }, t)).toBe(reasons.export_over_document);
    expect(describeError({ code: "invalid", message: "invalid value: path_is_document" }, t)).toBe(reasons.path_is_document);
    expect(describeError({ code: "invalid", message: "invalid value: project_not_found" }, t)).toBe(reasons.project_not_found);
    expect(describeError({ code: "invalid", message: "merge_name_invalid" }, t)).toBe(reasons.merge_name_invalid);
    expect(describeError({ code: "invalid", message: "no_check_run" }, t)).toBe(reasons.no_check_run);
    expect(describeError({ code: "invalid", message: "invalid value: schema_not_found: 영수증 v2" }, t)).toBe(reasons.schema_not_found);
  });

  it("translates the 0.8.0 reasons of the pipeline / document / project commands (`{ code: \"invalid\", message: <reason> }`)", () => {
    // The last enabled 일반 form cannot be switched off, deleted or retyped (`repo::keep_general`).
    expect(describeError({ code: "invalid", message: "schema_general_required" }, t)).toBe(reasons.schema_general_required);
    expect(describeError({ code: "db", message: "database error: Invalid parameter name: schema_general_required" }, t)).toBe(reasons.schema_general_required);
    // A table export to a folder / an empty path; a merge name that is no file name.
    expect(describeError({ code: "invalid", message: "export_path_invalid" }, t)).toBe(reasons.export_path_invalid);
    expect(describeError({ code: "invalid", message: "merge_name_invalid" }, t)).toBe(reasons.merge_name_invalid);
    // A deletion whose file another program holds open (DOC-12).
    expect(describeError({ code: "invalid", message: "file_in_use" }, t)).toBe(reasons.file_in_use);
    expect(describeError({ code: "invalid", message: "invalid value: file_in_use" }, t)).toBe(reasons.file_in_use);
    // 보완요청서: only passed / undetermined results selected; a selection of another check run.
    expect(describeError({ code: "invalid", message: "invalid value: findings_not_requestable" }, t)).toBe(reasons.findings_not_requestable);
    expect(describeError({ code: "invalid", message: "invalid value: findings_not_in_run" }, t)).toBe(reasons.findings_not_in_run);
    expect(reasons.schema_general_required).toContain("일반");
    expect(reasons.file_in_use).toContain("사용 중");
  });

  it("names what a `not_found` error is missing (`CoreError::NotFound(\"project <id>\")`)", () => {
    expect(describeError({ code: "not_found", message: "not found: project 3f2a9c" }, t)).toBe(reasons.project_not_found);
    // No copy for that noun: the class template with the raw reason.
    expect(describeError({ code: "not_found", message: "not found: page 7" }, t)).toBe(ko.errors.not_found.replace("{{message}}", "page 7"));
  });

  it("keeps the phrase reasons of the document commands", () => {
    expect(describeError({ code: "invalid", message: "nothing to import" }, t)).toBe(reasons.nothing_to_import);
    expect(describeError({ code: "invalid", message: "merge needs at least two documents" }, t)).toBe(reasons.merge_two);
    expect(describeError({ code: "invalid", message: "document_id required" }, t)).toBe(reasons.document_id_required);
  });

  it("falls back to the error class template, then to the generic one", () => {
    expect(describeError({ code: "invalid", message: "invalid value: word range 3..1" }, t)).toBe(ko.errors.invalid.replace("{{message}}", "word range 3..1"));
    expect(describeError({ code: "not_found", message: "not found: abc123" }, t)).toBe(ko.errors.not_found.replace("{{message}}", "abc123"));
    expect(describeError({ code: "workspace_locked", message: "workspace is in use by another instance (pid 7 on PC)", detail: { pid: 7, host: "PC" } }, t)).toBe(ko.errors.workspace_locked.replace("{{pid}}", "7").replace("{{host}}", "PC"));
    expect(describeError({ code: "some_new_code", message: "something broke" }, t)).toBe(ko.errors.generic.replace("{{message}}", "something broke"));
    expect(describeError("plain text", t)).toBe(ko.errors.generic.replace("{{message}}", "plain text"));
  });

  it("does not mistake ordinary words or paths for a reason", () => {
    expect(errorReasonKey({ code: "io", message: "I/O error at C:\\work\\no_text: access denied" })).toBeNull();
    expect(errorReasonKey({ code: "invalid", message: "invalid value: template_variable: bogus" })).toBeNull();
    expect(errorReasonKey({ code: "invalid", message: "invalid value: no_findings" })).toBe("errors.reasons.no_findings");
  });

  it("has English copy for every reason", () => {
    expect(Object.keys(en.errors.reasons).sort()).toEqual(Object.keys(reasons).sort());
    expect(describeError({ code: "invalid", message: "invalid value: no_findings" }, ((k: string, o?: Record<string, unknown>) => i18n.t(k, { ...o, lng: "en" })) as typeof t)).toBe(en.errors.reasons.no_findings);
  });
});
