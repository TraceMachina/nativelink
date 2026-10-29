import type * as React from "react";
import { cn } from "../lib/cn";
import { Eyebrow } from "./eyebrow";

interface FeatureCardProps {
  /** SVG path children; the card draws the 24×24 frame. */
  icon?: React.ReactNode;
  eyebrow?: string;
  title: string;
  body: string;
  /** A proof number for this feature, set in the card's top-right corner. */
  stat?: { value: string; label: string };
  className?: string;
}

/**
 * The one card anatomy for "a feature, and the number that proves it":
 * icon top-left, stat top-right, eyebrow, title, body. The Product pillars
 * and the home feature cards both use it, with or without the optional
 * parts, so the two pages read as one system.
 */
export function FeatureCard({ icon, eyebrow, title, body, stat, className }: FeatureCardProps) {
  const hasHeader = Boolean(icon || stat);
  return (
    <article
      className={cn(
        "group relative flex h-full flex-col overflow-hidden rounded-2xl border border-border bg-surface p-8 transition-all hover:border-brand/40 hover:shadow-[0_24px_60px_-30px_rgb(var(--nl-color-brand)/0.4)]",
        className,
      )}
    >
      {hasHeader ? (
        <div className="mb-6 flex items-start justify-between gap-4">
          {icon ? (
            <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl bg-brand-soft text-brand transition-colors group-hover:bg-brand group-hover:text-brand-foreground">
              <svg
                width="22"
                height="22"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.6"
                aria-hidden="true"
              >
                {icon}
              </svg>
            </div>
          ) : (
            <span />
          )}
          {stat ? (
            <div className="text-right">
              <div className="font-mono text-2xl font-semibold tracking-tight text-brand">
                {stat.value}
              </div>
              <div className="font-mono text-[10px] uppercase tracking-widest text-muted">
                {stat.label}
              </div>
            </div>
          ) : null}
        </div>
      ) : null}
      {eyebrow ? (
        <Eyebrow tone="muted" className="text-[10px]">
          {eyebrow}
        </Eyebrow>
      ) : null}
      <h3
        className={cn(
          "text-2xl font-semibold leading-tight tracking-tight text-foreground",
          eyebrow && "mt-2",
        )}
      >
        {title}
      </h3>
      <p className="mt-4 text-[15px] leading-relaxed text-muted-foreground">{body}</p>
    </article>
  );
}
