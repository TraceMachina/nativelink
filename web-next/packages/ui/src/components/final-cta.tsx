import { Button } from "./button";
import { Eyebrow } from "./eyebrow";
import { cn } from "../lib/cn";

interface FinalCTAProps {
  eyebrow?: string;
  title: string;
  body?: string;
  primaryLabel?: string;
  primaryHref?: string;
  secondaryLabel?: string;
  secondaryHref?: string;
  className?: string;
}

export function FinalCTA({
  eyebrow = "Ship faster",
  title,
  body,
  primaryLabel,
  primaryHref,
  secondaryLabel,
  secondaryHref,
  className,
}: FinalCTAProps) {
  return (
    <section
      className={cn(
        "relative overflow-hidden border-t border-[rgb(var(--nl-color-accent-line))]/40",
        className,
      )}
    >
      <div className="absolute inset-x-0 -top-32 mx-auto h-[400px] w-[600px] max-w-full rounded-full bg-[radial-gradient(ellipse_at_top,rgba(114,71,255,0.08),transparent_70%)]" />
      <div className="relative mx-auto flex w-full max-w-[860px] flex-col items-center gap-6 px-6 py-24 text-center">
        <Eyebrow>{eyebrow}</Eyebrow>
        <h2 className="text-3xl font-bold leading-[1.15] tracking-tight md:text-5xl">{title}</h2>
        {body ? (
          <p className="max-w-[600px] text-base leading-relaxed text-muted md:text-lg">{body}</p>
        ) : null}
        {(primaryLabel || secondaryLabel) && (
          <div className="mt-2 flex flex-wrap items-center justify-center gap-3">
            {primaryLabel && primaryHref && (
              <Button size="lg" asChild>
                <a href={primaryHref}>{primaryLabel}</a>
              </Button>
            )}
            {secondaryLabel && secondaryHref && (
              <Button size="lg" variant="outline" asChild>
                <a href={secondaryHref}>{secondaryLabel}</a>
              </Button>
            )}
          </div>
        )}
      </div>
    </section>
  );
}
