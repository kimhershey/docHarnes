import { Outlet } from "react-router";

import { Toaster } from "@/components/ui/toaster";
import { WorkspaceDialog } from "@/features/workspace/WorkspaceDialog";
import { DndLayer } from "@/lib/dnd";

import { useGlobalShortcuts } from "../hooks/useGlobalShortcuts";
import { CommandPalette } from "./CommandPalette";
import { Sidebar } from "./Sidebar";
import { StatusBar } from "./StatusBar";
import { TopBar } from "./TopBar";

export function AppShell() {
  useGlobalShortcuts();
  return (
    <div className="flex h-full flex-col bg-bg text-fg" data-testid="app-shell">
      <div className="flex min-h-0 flex-1">
        <Sidebar />
        <div className="flex min-w-0 flex-1 flex-col">
          <TopBar />
          <main className="min-h-0 flex-1 overflow-y-auto" id="main">
            <Outlet />
          </main>
        </div>
      </div>
      <StatusBar />
      <CommandPalette />
      <WorkspaceDialog />
      <Toaster />
      <DndLayer />
    </div>
  );
}
