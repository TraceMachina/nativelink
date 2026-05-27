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
        "relative overflow-hidden border-t border-border/60 bg-brand-glow",
        className,
      )}
    >
      <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(ellipse_at_center_top,rgb(var(--nl-color-brand)/0.18),transparent_60%)]" />
      <div className="relative mx-auto flex w-full max-w-[860px] flex-col items-center gap-6 px-6 py-24 text-center">
        <Eyebrow>{eyebrow}</Eyebrow>
        <h2 className="text-balance text-3xl font-semibold leading-[1.1] tracking-[-0.025em] md:text-5xl">
          {title}
        </h2>
        {body ? (
          <p className="max-w-[600px] text-base leading-relaxed text-muted-foreground md:text-lg">
            {body}
          </p>
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
