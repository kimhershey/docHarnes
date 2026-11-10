import { HashRouter, Navigate, Route, Routes, useLocation } from "react-router";

import { Toaster } from "@/components/ui/toaster";
import { AssistantPage } from "@/features/assistant/AssistantPage";
import { GeneratePage } from "@/features/generate/GeneratePage";
import { JobsPage } from "@/features/jobs/JobsPage";
import { KnowledgePage } from "@/features/knowledge/KnowledgePage";
import { DocumentViewer } from "@/features/library/DocumentViewer";
import { LibraryPage } from "@/features/library/LibraryPage";
import { OnboardingWizard } from "@/features/onboarding/OnboardingWizard";
import { OrganizePage } from "@/features/organize/OrganizePage";
import { ProjectDetailPage } from "@/features/projects/ProjectDetailPage";
import { ProjectsPage } from "@/features/projects/ProjectsPage";
import { ReviewPage } from "@/features/review/ReviewPage";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { settingsTabPath, type SettingsTab } from "@/features/settings/tabs";
import { SCREEN_ROUTES } from "@/lib/types";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

import { AppShell } from "./layout/AppShell";
import { AppBootstrap } from "./providers/AppBootstrap";

/** `/models` and `/schemas` (0.4 screens folded into the settings, plan §1) keep working: the query string travels along (`register=` of 양식 등록). */
function SettingsRedirect({ tab }: { tab: SettingsTab }) {
  const location = useLocation();
  return <Navigate to={settingsTabPath(tab, new URLSearchParams(location.search))} replace />;
}

/** `/search` → the assistant in "근거만 찾기" mode. */
function EvidenceRedirect() {
  const location = useLocation();
  const params = new URLSearchParams(location.search);
  params.set("evidence", "1");
  return <Navigate to={`${SCREEN_ROUTES.assistant}?${params.toString()}`} replace />;
}

function Gate() {
  const workspace = useWorkspace();
  const onboarding = useUiStore((s) => s.onboardingActive);
  if (!workspace || onboarding) {
    return (
      <>
        <OnboardingWizard />
        <Toaster />
      </>
    );
  }
  return (
    <Routes>
      <Route element={<AppShell />}>
        {/* 문서함 is the first screen (the former home's "오늘 할 일" lives in its strip). */}
        <Route index element={<Navigate to={SCREEN_ROUTES.library} replace />} />
        <Route path="library" element={<LibraryPage />} />
        <Route path="library/:id" element={<DocumentViewer />} />
        <Route path="search" element={<EvidenceRedirect />} />
        <Route path="assistant" element={<AssistantPage />} />
        <Route path="review" element={<ReviewPage />} />
        <Route path="schemas" element={<SettingsRedirect tab="schemas" />} />
        <Route path="knowledge" element={<KnowledgePage />} />
        <Route path="generate" element={<GeneratePage />} />
        <Route path="projects" element={<ProjectsPage />} />
        <Route path="projects/:id" element={<ProjectDetailPage />} />
        <Route path="organize" element={<OrganizePage />} />
        <Route path="jobs" element={<JobsPage />} />
        <Route path="models" element={<SettingsRedirect tab="models" />} />
        <Route path="settings" element={<SettingsPage />} />
        <Route path="*" element={<Navigate to={SCREEN_ROUTES.library} replace />} />
      </Route>
    </Routes>
  );
}

export function App() {
  return (
    <HashRouter>
      <AppBootstrap>
        <Gate />
      </AppBootstrap>
    </HashRouter>
  );
}
