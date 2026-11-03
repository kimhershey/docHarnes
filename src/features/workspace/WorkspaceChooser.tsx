import { AlertTriangle, FolderOpen, FolderSearch, Info } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { api, isTauri, pickFolder } from "@/lib/ipc";
import type { WorkspaceInspection, WorkspaceView } from "@/lib/types";
import { describeError } from "@/lib/errors";
import { errorCode } from "@/lib/utils";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

export interface WorkspaceChooserProps {
  mode: "open" | "create";
  onOpened: (ws: WorkspaceView) => void;
  showRecent?: boolean;
}

/** Folder picker + inspection + open/create (shared by onboarding and the dialog). */
export function WorkspaceChooser({ mode, onOpened, showRecent = true }: WorkspaceChooserProps) {
  const { t } = useTranslation();
  const recent = useSettingsStore((s) => s.settings?.workspaces ?? []);
  const open = useWorkspaceStore((s) => s.open);
  const create = useWorkspaceStore((s) => s.create);
  const forget = useWorkspaceStore((s) => s.forget);
  const toast = useUiStore((s) => s.toast);
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [inspection, setInspection] = useState<WorkspaceInspection | null>(null);
  const [busy, setBusy] = useState(false);
  const [lockedBy, setLockedBy] = useState<{ pid: number; host: string } | null>(null);

  useEffect(() => {
    const p = path.trim();
    setLockedBy(null);
    if (!p) {
      setInspection(null);
      return;
    }
    let cancelled = false;
    const handle = window.setTimeout(() => {
      void api
        .workspaceInspect(p)
        .then((i) => {
          if (!cancelled) setInspection(i);
        })
        .catch(() => {
          if (!cancelled) setInspection(null);
        });
    }, 200);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [path]);

  const browse = async () => {
    const chosen = await pickFolder(t("onboarding.workspace.path_label"), path || undefined);
    if (chosen) setPath(chosen);
  };

  const submit = async (forceUnlock = false) => {
    const p = path.trim();
    if (!p) return;
    setBusy(true);
    try {
      // Decide open vs. create from a fresh inspection (the debounced one may be stale or pending).
      const insp = inspection && inspection.path === p ? inspection : await api.workspaceInspect(p);
      if (insp.unsafe_location) return;
      const ws = insp.initialized ? await open(p, forceUnlock) : await create(p, name.trim() || null, null, forceUnlock);
      onOpened(ws);
    } catch (e) {
      if (errorCode(e) === "workspace_locked") {
        const d = (e as { detail?: { pid?: number; host?: string } }).detail;
        setLockedBy({ pid: d?.pid ?? 0, host: d?.host ?? "?" });
      } else {
        toast(describeError(e, t), "danger");
      }
    } finally {
      setBusy(false);
    }
  };

  const willCreate = inspection ? !inspection.initialized : mode === "create";
  const canSubmit = path.trim().length > 0 && !busy && !(inspection?.unsafe_location ?? false);

  return (
    <div className="flex flex-col gap-4" data-testid="workspace-chooser">
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="ws-path">{t("onboarding.workspace.path_label")}</Label>
        <div className="flex gap-2">
          <Input
            id="ws-path"
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder={t("onboarding.workspace.path_placeholder")}
            spellCheck={false}
            autoFocus
            data-testid="workspace-path"
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.nativeEvent.isComposing && canSubmit) void submit();
            }}
          />
          <Button variant="secondary" onClick={() => void browse()} disabled={!isTauri} title={isTauri ? undefined : t("onboarding.workspace.browser_hint")}>
            <FolderSearch />
            {t("common.browse")}
          </Button>
        </div>
        {!isTauri && <p className="text-xs text-fg-subtle">{t("onboarding.workspace.browser_hint")}</p>}
      </div>

      {inspection && (
        <div className="flex items-start gap-2 rounded-md border border-border bg-bg p-3 text-sm" data-testid="workspace-inspection">
          {inspection.unsafe_location || inspection.lock_state === "held_here" || inspection.lock_state === "held_elsewhere" ? (
            <AlertTriangle className="mt-0.5 size-4 shrink-0 text-warn" />
          ) : (
            <Info className="mt-0.5 size-4 shrink-0 text-info" />
          )}
          <div className="flex flex-col gap-1">
            {inspection.unsafe_location && <span className="text-warn">{t("onboarding.workspace.inspect_unsafe")}</span>}
            {!inspection.unsafe_location && !inspection.exists && <span>{t("onboarding.workspace.inspect_missing")}</span>}
            {!inspection.unsafe_location && inspection.exists && inspection.initialized && (
              <span>{t("onboarding.workspace.inspect_existing", { name: inspection.manifest?.name ?? "" })}</span>
            )}
            {!inspection.unsafe_location && inspection.exists && !inspection.initialized && <span>{t("onboarding.workspace.inspect_new")}</span>}
            {inspection.document_files > 0 && <span className="text-fg-muted">{t("onboarding.workspace.inspect_documents", { count: inspection.document_files })}</span>}
            {inspection.lock_state === "stale" && <span className="text-fg-muted">{t("onboarding.workspace.inspect_stale")}</span>}
            {(inspection.lock_state === "held_here" || inspection.lock_state === "held_elsewhere") && inspection.lock && (
              <span className="text-warn">{t("onboarding.workspace.inspect_locked", { pid: inspection.lock.pid, host: inspection.lock.host })}</span>
            )}
          </div>
        </div>
      )}

      {willCreate && (
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="ws-name">{t("onboarding.workspace.name_label")}</Label>
          <Input id="ws-name" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("onboarding.workspace.name_placeholder")} data-testid="workspace-name" />
        </div>
      )}

      {lockedBy && (
        <div className="flex items-center justify-between gap-2 rounded-md border border-warn bg-warn-soft p-3 text-sm">
          <span>{t("onboarding.workspace.inspect_locked", { pid: lockedBy.pid, host: lockedBy.host })}</span>
          <Button variant="outline" size="sm" onClick={() => void submit(true)}>
            {t("onboarding.workspace.force_unlock")}
          </Button>
        </div>
      )}

      <div className="flex items-center justify-end gap-2">
        <Button onClick={() => void submit()} disabled={!canSubmit} data-testid="workspace-submit">
          <FolderOpen />
          {willCreate ? t("onboarding.workspace.create_and_open") : t("onboarding.workspace.open")}
        </Button>
      </div>

      {showRecent && recent.length > 0 && (
        <div className="flex flex-col gap-1.5">
          <p className="text-xs font-semibold uppercase tracking-wide text-fg-subtle">{t("onboarding.workspace.recent")}</p>
          <ul className="flex flex-col divide-y divide-border rounded-md border border-border">
            {recent.map((w) => (
              <li key={w.path} className="flex items-center gap-2 px-3 py-2 text-sm">
                <button type="button" className="flex min-w-0 flex-1 flex-col items-start text-left hover:text-accent cursor-pointer" onClick={() => setPath(w.path)} data-testid="recent-workspace">
                  <span className="truncate font-medium">{w.name}</span>
                  <span className="truncate text-xs text-fg-subtle">{w.path}</span>
                </button>
                <Button variant="ghost" size="sm" onClick={() => void forget(w.path)}>
                  {t("settings.workspace.forget")}
                </Button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
