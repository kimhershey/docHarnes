import { Plus, Sparkles, Trash2 } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { invalidateProjects } from "@/lib/queries";
import { isComposing } from "@/lib/shortcuts";
import type { ProjectDetail, Rule, RuleDraft } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

import { RuleGrid, RuleSummary } from "./RuleGrid";
import { newRule, ruleComplete } from "./rules";

/** 규칙 tab: natural language → `project_define_rule` → editable rule grid → save; list with enable / delete. */
export function RulesTab({ detail }: { detail: ProjectDetail }) {
  const { t, i18n } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const id = detail.project.id;
  const [text, setText] = useState("");
  const [defining, setDefining] = useState(false);
  const [draft, setDraft] = useState<RuleDraft | null>(null);
  const [saving, setSaving] = useState(false);

  const define = async () => {
    const description = text.trim();
    if (!description) return;
    setDefining(true);
    try {
      setDraft(await api.projectDefineRule(id, description));
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setDefining(false);
    }
  };
  const save = async (rule: Rule) => {
    setSaving(true);
    try {
      await api.projectSaveRule(id, rule);
      invalidateProjects([id]);
      toast(t("projects.rules.saved"), "ok");
      setDraft(null);
      setText("");
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setSaving(false);
    }
  };
  const remove = async (ruleId: string) => {
    try {
      await api.projectDeleteRule(id, ruleId);
      invalidateProjects([id]);
      toast(t("projects.rules.deleted"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const toggle = async (rule: Rule, enabled: boolean) => {
    try {
      await api.projectSaveRule(id, { ...rule, enabled });
      invalidateProjects([id]);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  return (
    <div className="flex flex-col gap-4" data-testid="project-rules">
      <section className="flex flex-col gap-2 rounded-lg border border-border bg-bg-elevated p-3">
        <Label htmlFor="rule-nl">{t("projects.rules.nl_label")}</Label>
        <Textarea
          id="rule-nl"
          rows={2}
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder={t("projects.rules.nl_placeholder")}
          lang={i18n.language}
          data-testid="rule-nl-input"
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              if (isComposing(e)) return;
              e.preventDefault();
              void define();
            }
          }}
        />
        <div className="flex flex-wrap items-center gap-2">
          <Button size="sm" onClick={() => void define()} disabled={defining || !text.trim()} data-testid="rule-define">
            <Sparkles />
            {t("projects.rules.define")}
          </Button>
          <Button size="sm" variant="outline" onClick={() => setDraft({ rule: newRule(text.trim()), confidence: 1, source: "manual", issues: [], unresolved: [] })} data-testid="rule-manual">
            <Plus />
            {t("projects.rules.manual")}
          </Button>
          {defining && (
            <span className="flex items-center gap-1 text-xs text-fg-muted" role="status">
              <Spinner className="size-3.5" />
              {t("projects.rules.defining")}
            </span>
          )}
        </div>
        {draft && (
          <div className="flex flex-col gap-2" data-testid="rule-draft">
            <div className="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
              <span className="font-medium text-fg">{t("projects.rules.grid_title")}</span>
              {draft.source !== "manual" && <span>{t("projects.rules.confidence", { value: draft.confidence.toFixed(2), source: t(`projects.rules.source.${draft.source === "model" ? "model" : "rules"}`) })}</span>}
            </div>
            {draft.issues.length > 0 && (
              <ul className="rounded-md bg-warn-soft px-2 py-1.5 text-xs text-warn" data-testid="rule-draft-issues">
                {draft.issues.map((i, n) => (
                  <li key={n}>⚠ {i}</li>
                ))}
              </ul>
            )}
            <RuleGrid rule={draft.rule} onChange={(rule) => setDraft({ ...draft, rule })} unresolvedNames={draft.unresolved} testId="rule-grid" />
            <div className="flex items-center gap-2">
              <Button size="sm" onClick={() => void save(draft.rule)} disabled={saving || !ruleComplete(draft.rule)} data-testid="rule-save">
                {t("projects.rules.save")}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setDraft(null)} data-testid="rule-discard">
                {t("projects.rules.discard")}
              </Button>
              {!ruleComplete(draft.rule) && <span className="text-xs text-danger">{t("projects.rules.incomplete")}</span>}
            </div>
          </div>
        )}
      </section>
      <section className="flex flex-col gap-2" aria-labelledby="rules-list-title">
        <h3 id="rules-list-title" className="text-sm font-semibold">
          {t("projects.rules.title")} ({detail.rules.length})
        </h3>
        {detail.rules.length === 0 ? (
          <p className="text-sm text-fg-muted">{t("projects.rules.empty")}</p>
        ) : (
          <ul className="divide-y divide-border rounded-lg border border-border bg-bg-elevated" data-testid="rule-list">
            {detail.rules.map((r) => (
              <li key={r.id} className="flex items-start gap-3 px-3 py-2" data-testid={`rule-${r.id}`} data-rule-enabled={r.enabled}>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2 text-sm">
                    <span className="truncate font-medium">{r.description}</span>
                    {r.required ? <Badge variant="outline">{t("projects.rules.required")}</Badge> : <Badge variant="muted">{t("projects.rules.optional")}</Badge>}
                  </div>
                  <RuleSummary rule={r} className="mt-0.5" />
                </div>
                <label className="flex items-center gap-1 text-xs text-fg-muted">
                  <Switch checked={r.enabled} onCheckedChange={(v) => void toggle(r, v)} aria-label={t("projects.rules.enabled")} data-testid="rule-enabled" />
                  {t("projects.rules.enabled")}
                </label>
                <Button size="icon-sm" variant="ghost" className="text-danger" onClick={() => void remove(r.id)} aria-label={t("projects.rules.delete")} data-testid="rule-delete">
                  <Trash2 />
                </Button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
