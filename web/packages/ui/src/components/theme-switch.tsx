"use client";

import type * as React from "react";
import { cn } from "../lib/cn";
import { type ThemePreference, useTheme } from "./theme-provider";

interface ThemeSwitchProps extends React.FieldsetHTMLAttributes<HTMLFieldSetElement> {}

const options: { value: ThemePreference; label: string; icon: React.ReactNode }[] = [
  {
    value: "light",
    label: "Light theme",
    icon: (
      <>
        <circle cx="12" cy="12" r="4" />
        <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
      </>
    ),
  },
  {
    value: "system",
    label: "Follow the system theme",
    icon: (
      <>
        <rect x="3" y="4" width="18" height="12" rx="2" />
        <path d="M8 20h8M12 16v4" />
      </>
    ),
  },
  {
    value: "dark",
    label: "Dark theme",
    icon: <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />,
  },
];

/** Three-way theme control: light, follow the system, dark. The header's
 *  ThemeToggle flips between light and dark; this one also exposes the
 *  system option, which is what a footer control should offer. */
export function ThemeSwitch({ className, ...props }: ThemeSwitchProps) {
  const { preference, setPreference, mounted } = useTheme();

  return (
    <fieldset
      className={cn(
        "inline-flex items-center rounded-lg border border-border bg-surface p-0.5",
        className,
      )}
      {...props}
    >
      <legend className="sr-only">Theme</legend>
      {options.map((option) => {
        const active = mounted && preference === option.value;
        return (
          <button
            key={option.value}
            type="button"
            aria-pressed={active}
            aria-label={option.label}
            title={option.label}
            onClick={() => setPreference(option.value)}
            className={cn(
              "inline-flex h-9 w-10 cursor-pointer items-center justify-center rounded-md transition-colors",
              "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60",
              active
                ? "bg-foreground/[0.07] text-foreground"
                : "text-muted hover:bg-foreground/[0.04] hover:text-foreground",
            )}
          >
            <svg
              width="16"
              height="16"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.75"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              {option.icon}
            </svg>
          </button>
        );
      })}
    </fieldset>
  );
}
