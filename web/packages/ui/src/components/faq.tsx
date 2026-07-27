"use client";

import * as React from "react";
import { cn } from "../lib/cn";

export interface FAQItem {
  q: string;
  a: React.ReactNode;
}

interface FAQProps {
  items: FAQItem[];
  defaultOpenIndex?: number;
  className?: string;
}

export function FAQ({ items, defaultOpenIndex, className }: FAQProps) {
  const [open, setOpen] = React.useState<number | null>(
    typeof defaultOpenIndex === "number" ? defaultOpenIndex : null,
  );

  return (
    <ul className={cn("divide-y divide-border overflow-hidden rounded-xl border border-border bg-surface/40", className)}>
      {items.map((item, i) => {
        const isOpen = open === i;
        return (
          <li key={item.q}>
            <button
              type="button"
              aria-expanded={isOpen}
              onClick={() => setOpen(isOpen ? null : i)}
              className={cn(
                "flex w-full cursor-pointer items-center justify-between gap-4 px-6 py-5 text-left transition-colors",
                "hover:bg-foreground/[0.02]",
                isOpen && "bg-brand-soft/30",
              )}
            >
              <span className={cn(
                "text-base font-medium md:text-lg",
                isOpen ? "text-foreground" : "text-foreground/90",
              )}>
                {item.q}
              </span>
              <span
                aria-hidden="true"
                className={cn(
                  "flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-xs transition-all",
                  isOpen
                    ? "rotate-45 bg-brand text-brand-foreground"
                    : "bg-foreground/5 text-muted-foreground",
                )}
              >
                +
              </span>
            </button>
            <div
              className={cn(
                "grid overflow-hidden transition-[grid-template-rows] duration-200 ease-out",
                isOpen ? "grid-rows-[1fr] pb-5" : "grid-rows-[0fr]",
              )}
            >
              <div className="min-h-0">
                <div className="max-w-[68ch] px-6 pt-3 text-[15px] leading-relaxed text-muted-foreground">
                  {item.a}
                </div>
              </div>
            </div>
          </li>
        );
      })}
    </ul>
  );
}
