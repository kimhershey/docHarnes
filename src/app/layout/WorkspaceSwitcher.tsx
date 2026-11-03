import { ChevronDown, FolderOpen, FolderPlus, LogOut } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { errorMessage } from "@/lib/utils";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

export function WorkspaceSwitcher() {
  const { t } = useTranslation();
  const current = useWorkspaceStore((s) => s.current);
  const open = useWorkspaceStore((s) => s.open);
  const close = useWorkspaceStore((s) => s.close);
  const recent = useSettingsStore((s) => s.settings?.workspaces ?? []);
  const openDialog = useUiStore((s) => s.openWorkspaceDialog);
  const toast = useUiStore((s) => s.toast);

  const switchTo = async (path: string) => {
    try {
      await open(path);
    } catch (e) {
      toast(errorMessage(e), "danger");
    }
  };

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" className="max-w-64 gap-2 px-2" data-testid="workspace-switcher" title={current?.path}>
          <FolderOpen className="text-accent" />
          <span className="truncate font-medium">{current?.manifest.name ?? t("topbar.no_workspace")}</span>
          <ChevronDown className="opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-80">
        {current && (
          <>
            <DropdownMenuLabel className="font-normal">
              <div className="truncate font-medium text-fg">{current.manifest.name}</div>
              <div className="truncate text-xs text-fg-subtle selectable">{current.path}</div>
            </DropdownMenuLabel>
            <DropdownMenuSeparator />
          </>
        )}
        {recent.filter((w) => w.path !== current?.path).length > 0 && (
          <>
            <DropdownMenuLabel>{t("topbar.recent")}</DropdownMenuLabel>
            {recent
              .filter((w) => w.path !== current?.path)
              .slice(0, 6)
              .map((w) => (
                <DropdownMenuItem key={w.path} onSelect={() => void switchTo(w.path)}>
                  <FolderOpen />
                  <span className="flex-1 truncate">{w.name}</span>
                </DropdownMenuItem>
              ))}
            <DropdownMenuSeparator />
          </>
        )}
        <DropdownMenuItem onSelect={() => openDialog("open")}>
          <FolderOpen />
          {t("topbar.open_other")}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => openDialog("create")}>
          <FolderPlus />
          {t("topbar.new_workspace")}
        </DropdownMenuItem>
        {current && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => void close()} data-testid="workspace-close">
              <LogOut />
              {t("topbar.close_workspace")}
            </DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
