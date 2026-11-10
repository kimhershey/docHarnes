import { Check, Eye, FolderSearch, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";

import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";

/** Where the menu was opened and on which document (null = the empty space of the list). */
export interface LibraryContextRequest {
  x: number;
  y: number;
  docId: string | null;
}

/**
 * Right-click menu of 문서함 (0.5.0 plan §3): the maintenance actions that left the toolbar — 다시 렌더 (the
 * clicked document or the selection), 다시 검사 and the inbox watch switch — plus "문서 열기" on a card.
 * The anchor is an invisible point at the pointer position.
 */
export function LibraryContextMenu({
  request,
  onClose,
  inboxWatch,
  canRerender,
  onOpen,
  onRerender,
  onRescan,
  onToggleInbox,
}: {
  request: LibraryContextRequest | null;
  onClose: () => void;
  inboxWatch: boolean;
  canRerender: boolean;
  onOpen: (id: string) => void;
  onRerender: (docId: string | null) => void;
  onRescan: () => void;
  onToggleInbox: () => void;
}) {
  const { t } = useTranslation();
  return (
    <DropdownMenu
      open={request !== null}
      modal={false}
      onOpenChange={(o) => {
        if (!o) onClose();
      }}
    >
      <DropdownMenuTrigger asChild>
        <span aria-hidden="true" tabIndex={-1} style={{ position: "fixed", left: request?.x ?? 0, top: request?.y ?? 0, width: 0, height: 0 }} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" sideOffset={2} onCloseAutoFocus={(e) => e.preventDefault()} aria-label={t("library.context_menu")} data-testid="library-context-menu">
        {request?.docId && (
          <DropdownMenuItem onSelect={() => onOpen(request.docId!)} data-testid="library-context-open">
            <Eye />
            {t("library.actions.open_doc")}
          </DropdownMenuItem>
        )}
        <DropdownMenuItem disabled={!canRerender} onSelect={() => onRerender(request?.docId ?? null)} data-testid="library-context-rerender">
          <RefreshCw />
          {t("library.actions.rerender")}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={onRescan} data-testid="library-context-rescan">
          <FolderSearch />
          {t("library.rescan")}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={onToggleInbox} role="menuitemcheckbox" aria-checked={inboxWatch} data-testid="library-context-inbox-watch" data-checked={inboxWatch ? "true" : "false"}>
          {inboxWatch ? <Check /> : <span className="size-4" aria-hidden="true" />}
          {t("library.inbox_watch")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
