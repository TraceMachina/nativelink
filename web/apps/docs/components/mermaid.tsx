"use client";

import * as React from "react";

interface MermaidProps {
  children: React.ReactNode;
}

/** Read the active theme by inspecting <html data-theme> — works regardless
 *  of which theme provider sets it (our @nativelink/ui ThemeProvider on
 *  marketing, next-themes via Fumadocs in docs). */
function useThemeAttribute(): "light" | "dark" {
  const [theme, setTheme] = React.useState<"light" | "dark">("dark");

  React.useEffect(() => {
    const read = () => {
      const value = document.documentElement.getAttribute("data-theme");
      setTheme(value === "light" ? "light" : "dark");
    };
    read();
    const observer = new MutationObserver(read);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    return () => observer.disconnect();
  }, []);

  return theme;
}

/** Renders a Mermaid diagram from its source. Themed to match the docs UI
 *  in both light and dark modes; the mermaid runtime is lazy-imported so
 *  it only ships on pages that have a diagram. */
export function Mermaid({ children }: MermaidProps) {
  const theme = useThemeAttribute();
  const reactId = React.useId().replace(/[^a-z0-9]/gi, "");
  const [svg, setSvg] = React.useState<string>("");
  const [error, setError] = React.useState<string | null>(null);

  const chart = React.useMemo(() => {
    if (typeof children === "string") return children.trim();
    return React.Children.toArray(children)
      .map((c) => (typeof c === "string" ? c : ""))
      .join("")
      .trim();
  }, [children]);

  React.useEffect(() => {
    let cancelled = false;
    (async () => {
      const mermaid = (await import("mermaid")).default;
      mermaid.initialize({
        startOnLoad: false,
        securityLevel: "loose",
        theme: theme === "dark" ? "dark" : "default",
        themeVariables: {
          fontFamily: "var(--nl-font-sans), ui-sans-serif, system-ui, -apple-system, sans-serif",
          fontSize: "14px",
          primaryColor: theme === "dark" ? "#221C3F" : "#EBE4FF",
          primaryTextColor: theme === "dark" ? "#F5F4F8" : "#111113",
          primaryBorderColor: theme === "dark" ? "#9C7CFF" : "#7247FF",
          secondaryColor: theme === "dark" ? "#121118" : "#FFFFFF",
          tertiaryColor: theme === "dark" ? "#0A090E" : "#FAF8F5",
          lineColor: theme === "dark" ? "#9C7CFF" : "#7247FF",
          textColor: theme === "dark" ? "#F5F4F8" : "#111113",
          mainBkg: theme === "dark" ? "#221C3F" : "#EBE4FF",
          actorBkg: theme === "dark" ? "#221C3F" : "#EBE4FF",
          actorBorder: theme === "dark" ? "#9C7CFF" : "#7247FF",
          actorTextColor: theme === "dark" ? "#F5F4F8" : "#111113",
          signalColor: theme === "dark" ? "#BCA2FF" : "#5832DC",
          signalTextColor: theme === "dark" ? "#F5F4F8" : "#111113",
          labelBoxBkgColor: theme === "dark" ? "#0A090E" : "#FAF8F5",
          labelBoxBorderColor: theme === "dark" ? "#9C7CFF" : "#7247FF",
          labelTextColor: theme === "dark" ? "#F5F4F8" : "#111113",
        },
      });
      try {
        const { svg: rendered } = await mermaid.render(`mermaid-${reactId}`, chart);
        if (!cancelled) setSvg(rendered);
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [chart, reactId, theme]);

  if (error) {
    return (
      <div className="my-6 rounded-xl border border-amber-500/40 bg-amber-50/40 p-4 text-sm dark:bg-amber-500/10">
        <p className="font-mono text-xs uppercase tracking-widest text-amber-700 dark:text-amber-300">
          Mermaid render error
        </p>
        <pre className="mt-2 whitespace-pre-wrap text-amber-700 dark:text-amber-300">{error}</pre>
      </div>
    );
  }

  return (
    <div
      role="img"
      aria-label="Diagram"
      className="my-8 flex justify-center overflow-x-auto rounded-xl border border-border bg-surface-elevated/40 p-6 [&_svg]:max-w-full [&_svg]:h-auto"
      // biome-ignore lint/security/noDangerouslySetInnerHtml: SVG is produced by mermaid from MDX content we author; not user input
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}
