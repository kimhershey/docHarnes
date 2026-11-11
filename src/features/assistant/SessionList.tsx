import { MessageSquare, MoreHorizontal, Pencil, Plus, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { ChatSessionSummary } from "@/lib/types";
import { cn } from "@/lib/utils";

/** "방금" / "5분 전" / "3일 전" … in the UI language (dates beyond a month are shown as such). */
export function relativeTime(iso: string, language: string, now = Date.now()): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const seconds = Math.round((then - now) / 1000);
  const abs = Math.abs(seconds);
  let fmt: Intl.RelativeTimeFormat;
  try {
    fmt = new Intl.RelativeTimeFormat(language, { numeric: "auto" });
  } catch {
    fmt = new Intl.RelativeTimeFormat("en", { numeric: "auto" });
  }
  if (abs < 60) return fmt.format(0, "second");
  if (abs < 3600) return fmt.format(Math.round(seconds / 60), "minute");
  if (abs < 86_400) return fmt.format(Math.round(seconds / 3600), "hour");
  if (abs < 86_400 * 31) return fmt.format(Math.round(seconds / 86_400), "day");
  return new Date(then).toLocaleDateString(language);
}

interface Props {
  sessions: ChatSessionSummary[];
  activeId: string | null;
  busy?: boolean;
  onNew: () => void;
  onSelect: (id: string) => void;
  onRename: (id: string, title: string) => void;
  onRemove: (id: string) => void;
}

/** Title of a session as shown in the rail / picker (an untitled one gets the placeholder). */
function titleOf(s: ChatSessionSummary, untitled: string): string {
  return s.title.trim() || untitled;
}

/**
 * The chat sessions of the workspace (0.7.0 §1.6): a left rail on wide windows (new chat, rows
 * with rename / delete), a header picker below `md`. Sizes are rem-based like the library's folder rail.
 */
export function SessionList({ sessions, activeId, busy, onNew, onSelect, onRename, onRemove }: Props) {
  const { t, i18n } = useTranslation();
  const [renaming, setRenaming] = useState<{ id: string; value: string } | null>(null);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  // Set by the rename item: the closing menu must not move the focus back to its trigger (that blur would commit at once).
  const renameStarting = useRef(false);
  const untitled = t("assistant.sessions.untitled");

  useEffect(() => {
    if (renaming) window.setTimeout(() => inputRef.current?.select(), 0);
  }, [renaming?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  const commitRename = () => {
    if (!renaming) return;
    const value = renaming.value.trim();
    const current = sessions.find((s) => s.id === renaming.id);
    if (value && current && value !== current.title) onRename(renaming.id, value);
    setRenaming(null);
  };

  const askRemove = (s: ChatSessionSummary) =>
    setConfirm({
      title: t("assistant.sessions.delete"),
      description: t("assistant.sessions.delete_confirm", { title: titleOf(s, untitled) }),
      confirmLabel: t("assistant.sessions.delete"),
      danger: true,
      onConfirm: () => onRemove(s.id),
    });

  return (
    <nav className="hidden w-56 shrink-0 flex-col border-r border-border p-3 md:flex" aria-label={t("assistant.sessions.title")} data-testid="assistant-sessions">
      <div className="mb-2 flex items-center gap-2 px-1">
        <h2 className="min-w-0 flex-1 truncate text-xs font-semibold uppercase tracking-wide text-fg-subtle">{t("assistant.sessions.title")}</h2>
        <Button variant="ghost" size="sm" onClick={onNew} disabled={busy} aria-label={t("assistant.sessions.new")} title={t("assistant.sessions.new")} data-testid="assistant-new-chat">
          <Plus className="size-4" aria-hidden="true" />
          {t("assistant.sessions.new")}
        </Button>
      </div>
      <ul className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto" role="list">
        {sessions.length === 0 && <li className="px-2 py-1 text-xs text-fg-muted">{t("assistant.sessions.empty")}</li>}
        {sessions.map((s) => {
          const active = s.id === activeId;
          const title = titleOf(s, untitled);
          return (
            <li
              key={s.id}
              className={cn("group flex items-center gap-1 rounded-md pr-1 text-sm", active ? "bg-accent-soft text-fg" : "text-fg-muted hover:bg-bg-sunken hover:text-fg")}
              data-testid={`assistant-session-${s.id}`}
              data-session-id={s.id}
              data-active={active ? "true" : "false"}
            >
              {renaming?.id === s.id ? (
                <Input
                  ref={inputRef}
                  value={renaming.value}
                  onChange={(e) => setRenaming({ id: s.id, value: e.target.value })}
                  onBlur={commitRename}
                  onKeyDown={(e) => {
                    if (e.nativeEvent.isComposing) return;
                    if (e.key === "Enter") {
                      e.preventDefault();
                      commitRename();
                    } else if (e.key === "Escape") {
                      e.preventDefault();
                      setRenaming(null);
                    }
                  }}
                  className="h-8 min-w-0 flex-1"
                  aria-label={t("assistant.sessions.rename")}
                  data-testid="assistant-session-rename-input"
                />
              ) : (
                <button
                  type="button"
                  className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 px-2 py-1.5 text-left"
                  onClick={() => onSelect(s.id)}
                  onDoubleClick={() => setRenaming({ id: s.id, value: s.title })}
                  aria-current={active ? "true" : undefined}
                  title={title}
                >
                  <MessageSquare className="size-3.5 shrink-0" aria-hidden="true" />
                  <span className="flex min-w-0 flex-1 flex-col">
                    <span className="truncate">{title}</span>
                    <span className="truncate text-[0.7rem] text-fg-subtle">
                      {relativeTime(s.updated_at, i18n.language)} · {t("assistant.sessions.messages", { count: s.message_count })}
                    </span>
                  </span>
                </button>
              )}
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    className={cn("shrink-0 cursor-pointer rounded-md p-1 text-fg-subtle hover:bg-bg-muted hover:text-fg", !active && "opacity-0 group-hover:opacity-100 focus:opacity-100")}
                    aria-label={t("assistant.sessions.menu", { title })}
                    data-testid={`assistant-session-menu-${s.id}`}
                  >
                    <MoreHorizontal className="size-4" aria-hidden="true" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent
                  align="end"
                  onCloseAutoFocus={(e) => {
                    if (!renameStarting.current) return;
                    renameStarting.current = false;
                    e.preventDefault();
                  }}
                >
                  <DropdownMenuItem
                    onSelect={() => {
                      renameStarting.current = true;
                      setRenaming({ id: s.id, value: s.title });
                    }}
                    data-testid="assistant-session-rename"
                  >
                    <Pencil aria-hidden="true" />
                    {t("assistant.sessions.rename")}
                  </DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => askRemove(s)} className="text-danger" data-testid="assistant-session-delete">
                    <Trash2 aria-hidden="true" />
                    {t("assistant.sessions.delete")}
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </li>
          );
        })}
      </ul>
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </nav>
  );
}

/** The session picker of narrow windows (the rail is hidden below `md`). */
export function SessionPicker({ sessions, activeId, onSelect }: Pick<Props, "sessions" | "activeId" | "onSelect">) {
  const { t } = useTranslation();
  const untitled = t("assistant.sessions.untitled");
  if (sessions.length === 0) return null;
  return (
    <Select value={activeId ?? undefined} onValueChange={onSelect}>
      <SelectTrigger className="h-8 w-44 md:hidden" aria-label={t("assistant.sessions.title")} data-testid="assistant-session-picker">
        <SelectValue placeholder={untitled} />
      </SelectTrigger>
      <SelectContent>
        {sessions.map((s) => (
          <SelectItem key={s.id} value={s.id}>
            {titleOf(s, untitled)}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
