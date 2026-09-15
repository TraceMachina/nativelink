"use client";

import * as React from "react";

/** The theme actually painted on the page. */
export type Theme = "light" | "dark";

/** What the visitor asked for. `system` follows the OS preference live. */
export type ThemePreference = Theme | "system";

interface ThemeContextValue {
  /** The resolved theme currently applied to `<html data-theme>`. */
  theme: Theme;
  /** The stored preference; `system` when the visitor has not chosen. */
  preference: ThemePreference;
  setTheme: (t: Theme) => void;
  setPreference: (p: ThemePreference) => void;
  toggle: () => void;
  mounted: boolean;
}

/** Shared with the docs app (next-themes, `storageKey: "nl-theme"`), so a
 *  choice made on either site carries over to the other. next-themes stores
 *  the literal string "system" for the follow-the-OS option; so do we. */
const STORAGE_KEY = "nl-theme";

const ThemeContext = React.createContext<ThemeContextValue>({
  theme: "dark",
  preference: "system",
  setTheme: () => {},
  setPreference: () => {},
  toggle: () => {},
  mounted: false,
});

function systemTheme(): Theme {
  if (typeof window === "undefined" || !window.matchMedia) return "dark";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function readStoredPreference(): ThemePreference {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "light" || stored === "dark") return stored;
  } catch {}
  return "system";
}

function applyTheme(theme: Theme) {
  document.documentElement.setAttribute("data-theme", theme);
  document.documentElement.style.colorScheme = theme;
}

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [preference, setPreferenceState] = React.useState<ThemePreference>("system");
  const [theme, setThemeState] = React.useState<Theme>("dark");
  const [mounted, setMounted] = React.useState(false);

  React.useEffect(() => {
    const stored = readStoredPreference();
    setPreferenceState(stored);
    setThemeState(stored === "system" ? systemTheme() : stored);
    setMounted(true);
  }, []);

  // While the preference is "system", follow the OS as it changes.
  React.useEffect(() => {
    if (preference !== "system" || typeof window === "undefined" || !window.matchMedia) return;
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      const next: Theme = query.matches ? "dark" : "light";
      applyTheme(next);
      setThemeState(next);
    };
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, [preference]);

  const setPreference = React.useCallback((next: ThemePreference) => {
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {}
    const resolved = next === "system" ? systemTheme() : next;
    applyTheme(resolved);
    setPreferenceState(next);
    setThemeState(resolved);
  }, []);

  const setTheme = React.useCallback((next: Theme) => setPreference(next), [setPreference]);

  const toggle = React.useCallback(() => {
    setPreference(theme === "dark" ? "light" : "dark");
  }, [theme, setPreference]);

  const value = React.useMemo(
    () => ({ theme, preference, setTheme, setPreference, toggle, mounted }),
    [theme, preference, setTheme, setPreference, toggle, mounted],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeContextValue {
  return React.useContext(ThemeContext);
}

/** Inline script to set the theme before paint — avoids FOUC. Inject via
 *  <script dangerouslySetInnerHTML={{ __html: themeInitScript }} /> in <head>.
 *  Anything stored other than "light" or "dark" (including "system", or
 *  nothing at all) resolves to the OS preference. */
export const themeInitScript = `(function(){try{var s=localStorage.getItem('nl-theme');var t=s==='light'||s==='dark'?s:(window.matchMedia&&window.matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light');document.documentElement.setAttribute('data-theme',t);document.documentElement.style.colorScheme=t;}catch(e){document.documentElement.setAttribute('data-theme','dark');}})();`;
