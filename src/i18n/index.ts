import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import en from "./en.json";
import ko from "./ko.json";
import type { Language } from "@/lib/types";

export const resources = {
  ko: { translation: ko },
  en: { translation: en },
} as const;

export function initI18n(language: Language = "ko"): typeof i18n {
  if (!i18n.isInitialized) {
    void i18n.use(initReactI18next).init({
      resources,
      lng: language,
      fallbackLng: "ko",
      interpolation: { escapeValue: false },
      returnNull: false,
    });
  } else if (i18n.language !== language) {
    void i18n.changeLanguage(language);
  }
  return i18n;
}

export default i18n;
