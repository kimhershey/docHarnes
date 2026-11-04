import { HashRouter, Route, Routes } from "react-router";

import { Toaster } from "@/components/ui/toaster";
import { AssistantPage } from "@/features/assistant/AssistantPage";
import { GeneratePage } from "@/features/generate/GeneratePage";
import { HomePage } from "@/features/home/HomePage";
import { JobsPage } from "@/features/jobs/JobsPage";
import { KnowledgePage } from "@/features/knowledge/KnowledgePage";
import { DocumentViewer } from "@/features/library/DocumentViewer";
import { LibraryPage } from "@/features/library/LibraryPage";
import { ModelsPage } from "@/features/models/ModelsPage";
import { OnboardingWizard } from "@/features/onboarding/OnboardingWizard";
import { OrganizePage } from "@/features/organize/OrganizePage";
import { ProjectsPage } from "@/features/projects/ProjectsPage";
import { ReviewPage } from "@/features/review/ReviewPage";
import { SchemasPage } from "@/features/schemas/SchemasPage";
import { SearchPage } from "@/features/search/SearchPage";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

import { AppShell } from "./layout/AppShell";
import { AppBootstrap } from "./providers/AppBootstrap";

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
        <Route index element={<HomePage />} />
        <Route path="library" element={<LibraryPage />} />
        <Route path="library/:id" element={<DocumentViewer />} />
        <Route path="search" element={<SearchPage />} />
        <Route path="assistant" element={<AssistantPage />} />
        <Route path="review" element={<ReviewPage />} />
        <Route path="schemas" element={<SchemasPage />} />
        <Route path="knowledge" element={<KnowledgePage />} />
        <Route path="generate" element={<GeneratePage />} />
        <Route path="projects" element={<ProjectsPage />} />
        <Route path="organize" element={<OrganizePage />} />
        <Route path="jobs" element={<JobsPage />} />
        <Route path="models" element={<ModelsPage />} />
        <Route path="settings" element={<SettingsPage />} />
        <Route path="*" element={<HomePage />} />
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
