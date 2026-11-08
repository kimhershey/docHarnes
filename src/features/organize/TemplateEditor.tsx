import { useMemo, useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";

import { insertAtCursor, unknownVariables } from "./template";

/**
 * Template input with variable chips that insert `{{var}}` at the caret. Unknown variables are
 * flagged locally (the backend rejects them with `template_variable` too; `error` carries that).
 */
export function TemplateEditor({
  id,
  label,
  value,
  onChange,
  disabled = false,
  variables,
  error = null,
  testId,
  children,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (next: string) => void;
  disabled?: boolean;
  variables: readonly string[];
  /** Backend error (unknown variables reported by the plan). */
  error?: string | null;
  testId: string;
  children?: ReactNode;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLInputElement | null>(null);
  const unknown = useMemo(() => unknownVariables(value), [value]);
  const message = error ?? (unknown.length > 0 ? t("organize.errors.template_variable", { vars: unknown.join(", ") }) : null);
  const insert = (name: string) => {
    const el = ref.current;
    const r = insertAtCursor(value, el?.selectionStart ?? null, el?.selectionEnd ?? null, `{{${name}}}`);
    onChange(r.value);
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(r.caret, r.caret);
    });
  };
  return (
    <div className="flex flex-col gap-2">
      <Label htmlFor={id}>{label}</Label>
      <Input
        ref={ref}
        id={id}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        disabled={disabled}
        spellCheck={false}
        autoComplete="off"
        className={cn("font-mono text-xs", message && "border-danger")}
        aria-invalid={message ? true : undefined}
        aria-describedby={message ? `${id}-error` : undefined}
        data-testid={testId}
      />
      <div className="flex flex-wrap gap-1" role="group" aria-label={t("organize.name.variables")}>
        {variables.map((v) => (
          <button
            type="button"
            key={v}
            disabled={disabled}
            onClick={() => insert(v)}
            className="rounded-full border border-border bg-bg-sunken px-2 py-0.5 text-xs hover:bg-accent-soft disabled:opacity-50"
            title={t("organize.name.insert", { label: t(`organize.variables.${v}`) })}
            data-testid={`${testId}-var-${v}`}
          >
            <span className="font-mono">{`{{${v}}}`}</span> <span className="text-fg-subtle">{t(`organize.variables.${v}`)}</span>
          </button>
        ))}
      </div>
      {message && (
        <p id={`${id}-error`} className="text-xs text-danger" role="alert" data-testid={`${testId}-error`}>
          {message}
        </p>
      )}
      {children}
    </div>
  );
}
