"use client";

import * as React from "react";
import { createPortal } from "react-dom";
import { cn } from "../lib/cn";
import { Button } from "./button";
import { Logo } from "./logo";
import { ThemeToggle } from "./theme-toggle";
import type { NavLink } from "./site-header";

interface MobileNavProps {
  links: NavLink[];
  ctaLabel: string;
  ctaHref: string;
  githubHref?: string;
}

export function MobileNav({
  links,
  ctaLabel,
  ctaHref,
  githubHref = "https://github.com/TraceMachina/nativelink",
}: MobileNavProps) {
  const [open, setOpen] = React.useState(false);
  const [mounted, setMounted] = React.useState(false);

  React.useEffect(() => {
    setMounted(true);
  }, []);

  React.useEffect(() => {
    if (!open) return;
    const original = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => {
      document.body.style.overflow = original;
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <>
      {/* Hamburger trigger — three bars that morph into an X */}
      <button
        type="button"
        aria-label={open ? "Close menu" : "Open menu"}
        aria-expanded={open}
        aria-controls="mobile-nav-panel"
        onClick={() => setOpen((v) => !v)}
        className={cn(
          "relative inline-flex h-11 w-11 cursor-pointer items-center justify-center rounded-full",
          "text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground",
          "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60",
          "md:hidden",
        )}
      >
        <span className="relative block h-4 w-5">
          <span
            className={cn(
              "absolute left-0 right-0 h-[1.75px] origin-center rounded-full bg-current transition-all duration-300",
              open ? "top-1/2 -translate-y-1/2 rotate-45" : "top-0",
            )}
          />
          <span
            className={cn(
              "absolute left-0 right-0 top-1/2 h-[1.75px] -translate-y-1/2 rounded-full bg-current transition-all duration-200",
              open ? "scale-x-0 opacity-0" : "scale-x-100 opacity-100",
            )}
          />
          <span
            className={cn(
              "absolute left-0 right-0 h-[1.75px] origin-center rounded-full bg-current transition-all duration-300",
              open ? "top-1/2 -translate-y-1/2 -rotate-45" : "bottom-0",
            )}
          />
        </span>
      </button>

      {/* Overlay — portal to document.body so the header's backdrop-filter
       *  stacking context doesn't trap it. */}
      {mounted
        ? createPortal(
            <div
              id="mobile-nav-panel"
              aria-hidden={!open}
              className={cn(
                "fixed inset-0 z-[100] md:hidden",
                open ? "pointer-events-auto" : "pointer-events-none",
              )}
            >
              {/* Backdrop */}
              <button
                type="button"
                aria-label="Close menu"
                tabIndex={open ? 0 : -1}
                onClick={() => setOpen(false)}
                className={cn(
                  "absolute inset-0 cursor-pointer bg-background/80 backdrop-blur-md transition-opacity duration-200",
                  open ? "opacity-100" : "opacity-0",
                )}
              />

              {/* Panel — slides down from top */}
              <nav
                aria-label="Mobile"
                className={cn(
                  "absolute inset-0 flex flex-col overflow-y-auto bg-background transition-all duration-300 ease-out",
                  open
                    ? "translate-y-0 opacity-100"
                    : "pointer-events-none -translate-y-2 opacity-0",
                )}
              >
                {/* Sticky header inside the menu — logo + theme + close */}
                <div className="sticky top-0 z-10 flex h-16 items-center border-b border-border/60 bg-background/80 px-6 backdrop-blur-md">
                  <a
                    href="/"
                    onClick={() => setOpen(false)}
                    aria-label="NativeLink — home"
                    className="inline-flex items-center"
                  >
                    <Logo size="md" />
                  </a>
                  <div className="ml-auto flex items-center gap-1">
                    <ThemeToggle />
                    <button
                      type="button"
                      aria-label="Close menu"
                      onClick={() => setOpen(false)}
                      className="inline-flex h-11 w-11 cursor-pointer items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground"
                    >
                      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" aria-hidden="true">
                        <path d="M18 6 6 18M6 6l12 12" strokeLinecap="round" />
                      </svg>
                    </button>
                  </div>
                </div>

                {/* Nav list */}
                <ul className="flex flex-col gap-1 px-6 py-8">
                  {links.map((link, i) => (
                    <li
                      key={link.href}
                      className={cn(
                        "transition-all duration-300 ease-out",
                        open
                          ? "translate-y-0 opacity-100"
                          : "translate-y-2 opacity-0",
                      )}
                      style={{ transitionDelay: open ? `${60 + i * 40}ms` : "0ms" }}
                    >
                      <a
                        href={link.href}
                        onClick={() => setOpen(false)}
                        className="group flex items-center justify-between rounded-xl px-4 py-4 text-2xl font-semibold tracking-tight text-foreground transition-colors hover:bg-brand-soft hover:text-brand"
                      >
                        <span>{link.label}</span>
                        <span
                          aria-hidden="true"
                          className="text-base text-muted transition-all group-hover:translate-x-1 group-hover:text-brand"
                        >
                          →
                        </span>
                      </a>
                    </li>
                  ))}
                </ul>

                {/* Footer — CTA + GitHub */}
                <div className="mt-auto border-t border-border/60 px-6 py-6">
                  <Button asChild size="lg" className="w-full">
                    <a href={ctaHref} onClick={() => setOpen(false)}>
                      {ctaLabel}
                    </a>
                  </Button>
                  <div className="mt-4 flex items-center justify-center gap-2">
                    <a
                      href={githubHref}
                      target="_blank"
                      rel="noreferrer"
                      aria-label="GitHub repository"
                      className="inline-flex h-11 w-11 cursor-pointer items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground"
                    >
                      <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
                        <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.56v-2.01c-3.2.7-3.87-1.36-3.87-1.36-.52-1.34-1.28-1.69-1.28-1.69-1.05-.71.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.76 2.7 1.25 3.36.96.1-.74.4-1.25.73-1.54-2.55-.29-5.23-1.28-5.23-5.68 0-1.25.45-2.28 1.19-3.08-.12-.29-.51-1.46.11-3.04 0 0 .97-.31 3.18 1.18.92-.26 1.91-.39 2.89-.39s1.97.13 2.89.39c2.21-1.49 3.18-1.18 3.18-1.18.62 1.58.23 2.75.11 3.04.74.8 1.19 1.83 1.19 3.08 0 4.41-2.69 5.39-5.25 5.67.41.36.78 1.06.78 2.14v3.17c0 .31.21.68.8.56 4.57-1.52 7.86-5.83 7.86-10.91C23.5 5.65 18.35.5 12 .5Z" />
                      </svg>
                    </a>
                  </div>
                </div>
              </nav>
            </div>,
            document.body,
          )
        : null}
    </>
  );
}
