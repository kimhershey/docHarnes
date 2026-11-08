import { Copy, Plus, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input, Textarea } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { docTypeLabel } from "@/features/pipeline/labels";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects, useDocTypes, useProjectTemplates } from "@/lib/queries";
import type { ProjectTemplate, Rule, TemplateInput } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

import { RuleGrid } from "./RuleGrid";
import { newRule, ruleComplete } from "./rules";

function toInput(tpl: ProjectTemplate | null): TemplateInput {
  return tpl ? { id: tpl.id, name_ko: tpl.name_ko, name_en: tpl.name_en, description_ko: tpl.description_ko, description_en: tpl.description_en, required_doc_types: [...tpl.required_doc_types], rules: structuredClone(tpl.rules) } : { id: null, name_ko: "", name_en: "", description_ko: "", description_en: "", required_doc_types: [], rules: [] };
}

/** Templates manager (§3.15): built-ins read-only (duplicate to edit), user templates with required types and rules. */
export function TemplatesDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  const { t, i18n } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: templates } = useProjectTemplates(open);
  const { data: docTypes } = useDocTypes(open);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [draft, setDraft] = useState<TemplateInput | null>(null);
  const [readOnly, setReadOnly] = useState(false);
  const [busy, setBusy] = useState(false);
  const ko = i18n.language.startsWith("ko");
  useEffect(() => {
    if (!open) {
      setSelectedId(null);
      setDraft(null);
    }
  }, [open]);
  const select = (tpl: ProjectTemplate) => {
    setSelectedId(tpl.id);
    setDraft(toInput(tpl));
    setReadOnly(tpl.builtin);
  };
  const create = () => {
    setSelectedId(null);
    setDraft(toInput(null));
    setReadOnly(false);
  };
  const duplicate = () => {
    if (!draft) return;
    setSelectedId(null);
    setDraft({ ...draft, id: null, name_ko: `${draft.name_ko} ${t("projects.templates.copy_suffix")}`, rules: draft.rules.map((r) => ({ ...r, id: crypto.randomUUID().replace(/-/g, "") })) });
    setReadOnly(false);
  };
  const save = async () => {
    if (!draft) return;
    setBusy(true);
    try {
      const saved = await api.projectTemplateSave(draft);
      invalidateProjects();
      toast(t("projects.templates.saved"), "ok");
      select(saved);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    if (!selectedId) return;
    setBusy(true);
    try {
      await api.projectTemplateDelete(selectedId);
      invalidateProjects();
      toast(t("projects.templates.deleted"), "ok");
      setSelectedId(null);
      setDraft(null);
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  const setRule = (i: number, r: Rule) => setDraft((d) => (d ? { ...d, rules: d.rules.map((x, n) => (n === i ? r : x)) } : d));
  // Like the rules tab: an incomplete rule (missing operand, bad tolerance) cannot be saved into the template.
  const incomplete = draft ? draft.rules.filter((r) => !ruleComplete(r)).length : 0;
  const toggleType = (slug: string) => setDraft((d) => (d ? { ...d, required_doc_types: d.required_doc_types.includes(slug) ? d.required_doc_types.filter((x) => x !== slug) : [...d.required_doc_types, slug] } : d));
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-4xl" closeLabel={t("common.close")} data-testid="templates-dialog">
        <DialogHeader>
          <DialogTitle>{t("projects.templates.title")}</DialogTitle>
          <DialogDescription>{t("projects.templates.hint")}</DialogDescription>
        </DialogHeader>
        <div className="grid max-h-[70vh] gap-4 md:grid-cols-[14rem_minmax(0,1fr)]">
          <div className="flex min-h-0 flex-col gap-2">
            <Button size="sm" variant="outline" onClick={create} data-testid="templates-new">
              <Plus />
              {t("projects.templates.new")}
            </Button>
            <ul className="min-h-0 flex-1 overflow-y-auto rounded-md border border-border text-sm">
              {(templates ?? []).map((tpl) => (
                <li key={tpl.id}>
                  <button type="button" className={cn("flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-bg-sunken cursor-pointer", tpl.id === selectedId && "bg-accent-soft")} onClick={() => select(tpl)} aria-pressed={tpl.id === selectedId} data-testid={`templates-item-${tpl.id}`}>
                    <span className="min-w-0 flex-1 truncate">{ko ? tpl.name_ko : tpl.name_en}</span>
                    {tpl.builtin && <Badge variant="muted">{t("projects.templates.builtin_badge")}</Badge>}
                  </button>
                </li>
              ))}
            </ul>
          </div>
          <div className="min-h-0 overflow-y-auto pr-1">
            {!draft ? (
              <p className="text-sm text-fg-muted">{t("projects.templates.select")}</p>
            ) : (
              <div className="flex flex-col gap-3" data-testid="templates-editor">
                {readOnly && <p className="rounded-md bg-info-soft px-2 py-1.5 text-xs text-info">{t("projects.templates.readonly")}</p>}
                <div className="grid gap-2 md:grid-cols-2">
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="tpl-name">{t("projects.templates.name")}</Label>
                    <Input id="tpl-name" value={draft.name_ko} onChange={(e) => setDraft({ ...draft, name_ko: e.target.value })} readOnly={readOnly} data-testid="templates-name" />
                  </div>
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor="tpl-name-en">{t("projects.templates.name_en")}</Label>
                    <Input id="tpl-name-en" value={draft.name_en ?? ""} onChange={(e) => setDraft({ ...draft, name_en: e.target.value })} readOnly={readOnly} />
                  </div>
                </div>
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="tpl-desc">{t("projects.templates.description")}</Label>
                  <Textarea id="tpl-desc" rows={2} value={draft.description_ko ?? ""} onChange={(e) => setDraft({ ...draft, description_ko: e.target.value })} readOnly={readOnly} />
                </div>
                <fieldset className="flex flex-col gap-1.5">
                  <legend className="text-sm font-medium">{t("projects.templates.required_types")}</legend>
                  <div className="flex flex-wrap gap-2" data-testid="templates-required-types">
                    {(docTypes ?? []).filter((d) => d.slug !== "other").map((d) => (
                      <label key={d.slug} className="flex items-center gap-1 rounded-full border border-border px-2 py-0.5 text-xs">
                        <input type="checkbox" checked={draft.required_doc_types.includes(d.slug)} onChange={() => toggleType(d.slug)} disabled={readOnly} />
                        {docTypeLabel(d.slug, docTypes, t, i18n.language)}
                      </label>
                    ))}
                  </div>
                </fieldset>
                <div className="flex flex-col gap-2">
                  <div className="flex items-center justify-between">
                    <span className="text-sm font-medium">{t("projects.templates.rules")}</span>
                    {!readOnly && (
                      <Button size="sm" variant="outline" onClick={() => setDraft({ ...draft, rules: [...draft.rules, newRule()] })} data-testid="templates-add-rule">
                        <Plus />
                        {t("projects.templates.add_rule")}
                      </Button>
                    )}
                  </div>
                  {draft.rules.length === 0 ? (
                    <p className="text-xs text-fg-subtle">{t("projects.templates.no_rules")}</p>
                  ) : (
                    draft.rules.map((r, i) => (
                      <div key={r.id} className="flex items-start gap-2" data-testid={`templates-rule-row-${i}`} data-rule-complete={ruleComplete(r)}>
                        <div className="flex min-w-0 flex-1 flex-col gap-1">
                          <RuleGrid rule={r} readOnly={readOnly} onChange={(x) => setRule(i, x)} testId={`templates-rule-${i}`} />
                          {!readOnly && !ruleComplete(r) && (
                            <span className="text-xs text-danger" data-testid={`templates-rule-incomplete-${i}`}>
                              {t("projects.rules.incomplete")}
                            </span>
                          )}
                        </div>
                        {!readOnly && (
                          <Button size="icon-sm" variant="ghost" className="text-danger" onClick={() => setDraft({ ...draft, rules: draft.rules.filter((_, n) => n !== i) })} aria-label={t("projects.rules.delete")}>
                            <Trash2 />
                          </Button>
                        )}
                      </div>
                    ))
                  )}
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  {!readOnly && (
                    <Button onClick={() => void save()} disabled={busy || !draft.name_ko.trim() || incomplete > 0} data-testid="templates-save">
                      {t("projects.templates.save")}
                    </Button>
                  )}
                  <Button variant="outline" onClick={duplicate} data-testid="templates-duplicate">
                    <Copy />
                    {t("projects.templates.duplicate")}
                  </Button>
                  {!readOnly && selectedId && (
                    <Button variant="ghost" className="text-danger" onClick={() => void remove()} disabled={busy} data-testid="templates-delete">
                      <Trash2 />
                      {t("projects.templates.delete")}
                    </Button>
                  )}
                  {!readOnly && incomplete > 0 && (
                    <span className="text-xs text-danger" role="status" data-testid="templates-incomplete">
                      {t("projects.templates.incomplete_rules", { count: incomplete })}
                    </span>
                  )}
                </div>
              </div>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
