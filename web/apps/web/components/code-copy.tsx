"use client";

import * as React from "react";

// Progressive enhancement for the copy buttons that lib/render-post.ts puts
// on every fenced code block. The buttons are in the server-rendered HTML;
// this component adds one delegated click handler for all of them.

const COPIED_MS = 1800;

async function writeClipboard(text: string): Promise<boolean> {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // Fall through to the legacy path (denied permission, insecure context).
    }
  }
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  textarea.remove();
  return ok;
}

export function CodeCopy() {
  React.useEffect(() => {
    const timers = new Map<HTMLButtonElement, number>();

    function onClick(event: MouseEvent) {
      const target = event.target as Element | null;
      const button = target?.closest<HTMLButtonElement>("button.code-copy");
      if (!button) return;
      const code = button.closest(".code-block")?.querySelector("pre");
      const text = code?.textContent ?? "";
      if (!text) return;

      void writeClipboard(text).then((ok) => {
        button.textContent = ok ? "Copied" : "Copy failed";
        button.dataset.copied = ok ? "true" : "false";
        window.clearTimeout(timers.get(button));
        timers.set(
          button,
          window.setTimeout(() => {
            button.textContent = "Copy";
            delete button.dataset.copied;
            timers.delete(button);
          }, COPIED_MS),
        );
      });
    }

    document.addEventListener("click", onClick);
    return () => {
      document.removeEventListener("click", onClick);
      for (const timer of timers.values()) window.clearTimeout(timer);
    };
  }, []);

  return null;
}
