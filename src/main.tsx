import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";

import "./styles/globals.css";
import { App } from "./app/App";
import { ThemeProvider } from "./app/providers/ThemeProvider";
import { TooltipProvider } from "./components/ui/tooltip";
import { initI18n } from "./i18n";
import { queryClient } from "./lib/query-client";
import { UI_MIRROR_KEY } from "./lib/theme";
import type { Language } from "./lib/types";

function initialLanguage(): Language {
  try {
    const raw = localStorage.getItem(UI_MIRROR_KEY);
    const lang = raw ? (JSON.parse(raw) as { language?: string }).language : undefined;
    return lang === "en" ? "en" : "ko";
  } catch {
    return "ko";
  }
}

initI18n(initialLanguage());

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <ThemeProvider>
        <TooltipProvider delayDuration={300}>
          <App />
        </TooltipProvider>
      </ThemeProvider>
    </QueryClientProvider>
  </StrictMode>,
);
