"use client";

import * as React from "react";
import { cn } from "../lib/cn";

interface VideoFacadeProps {
  youtubeId: string;
  /** What the video is, for the poster headline and the player's accessible title. */
  title: string;
  /** One line under the headline on the poster. */
  subtitle?: string;
  /** Running time shown on the poster, e.g. "1:30". */
  duration?: string;
  eyebrow?: string;
  /**
   * A designed poster image. Without one the poster is drawn from the design
   * tokens (brand gradient, dot grid, type), so it is on-brand by construction.
   */
  posterSrc?: string;
  className?: string;
}

/**
 * A designed poster with a brand play button in place of the stock YouTube
 * cover. Nothing from YouTube loads until the visitor presses play; then the
 * poster is swapped for the privacy-enhanced embed, autoplaying.
 */
export function VideoFacade({
  youtubeId,
  title,
  subtitle,
  duration,
  eyebrow = "Watch",
  posterSrc,
  className,
}: VideoFacadeProps) {
  const [playing, setPlaying] = React.useState(false);

  return (
    <div
      className={cn(
        "relative aspect-video w-full overflow-hidden rounded-2xl border border-border bg-surface shadow-[0_30px_80px_-25px_rgb(0_0_0_/_0.25)]",
        className,
      )}
    >
      {playing ? (
        <iframe
          src={`https://www.youtube-nocookie.com/embed/${youtubeId}?autoplay=1&rel=0`}
          title={title}
          allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture; web-share"
          referrerPolicy="strict-origin-when-cross-origin"
          allowFullScreen
          className="absolute inset-0 h-full w-full"
        />
      ) : (
        <button
          type="button"
          onClick={() => setPlaying(true)}
          aria-label={`Play video: ${title}`}
          className="group absolute inset-0 flex cursor-pointer flex-col justify-between p-6 text-left text-brand-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-brand-foreground/80 md:p-8"
        >
          {posterSrc ? (
            <img src={posterSrc} alt="" className="absolute inset-0 h-full w-full object-cover" />
          ) : (
            <>
              <span className="absolute inset-0 bg-gradient-to-br from-brand via-brand to-brand-strong" />
              <span className="absolute inset-0 bg-dot-grid opacity-25 [mask-image:radial-gradient(ellipse_at_top_left,black_30%,transparent_75%)]" />
              <span className="absolute -right-24 -top-24 h-72 w-72 rounded-full bg-background/10 blur-3xl" />
            </>
          )}

          <span className="relative flex items-center gap-2 font-mono text-[11px] uppercase tracking-[0.18em]">
            <span className="rounded-full border border-brand-foreground/30 bg-background/10 px-2.5 py-1 backdrop-blur-sm">
              {eyebrow}
            </span>
            {duration ? <span className="opacity-80">{duration}</span> : null}
          </span>

          <span className="relative mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-background text-brand shadow-[0_12px_40px_-8px_rgb(0_0_0_/_0.45)] transition-transform duration-300 group-hover:scale-105 md:h-20 md:w-20">
            <svg width="26" height="26" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M8 5.5v13l11-6.5-11-6.5Z" />
            </svg>
          </span>

          <span className="relative block">
            <span className="block text-balance text-xl font-semibold leading-tight tracking-[-0.02em] md:text-2xl">
              {title}
            </span>
            {subtitle ? (
              <span className="mt-1.5 block text-sm leading-relaxed opacity-85">{subtitle}</span>
            ) : null}
          </span>
        </button>
      )}
    </div>
  );
}
