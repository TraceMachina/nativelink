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
    <ul className={cn("divide-y-2 divide-border border-y-2 border-border", className)}>
      {items.map((item, i) => {
        const isOpen = open === i;
        return (
          <li key={item.q}>
            <button
              type="button"
              aria-expanded={isOpen}
              onClick={() => setOpen(isOpen ? null : i)}
              className="flex w-full items-center justify-between gap-4 py-5 text-left transition-colors hover:bg-foreground/[0.02]"
            >
              <span className="font-mono text-base font-medium text-foreground md:text-lg">
                {item.q}
              </span>
              <span
                aria-hidden="true"
                className={cn(
                  "shrink-0 font-mono text-base text-muted transition-transform",
                  isOpen && "rotate-45",
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
                <div className="max-w-[68ch] text-base leading-relaxed text-muted-foreground">
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
