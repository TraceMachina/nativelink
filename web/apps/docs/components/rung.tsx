import { LADDER_LENGTH, rungAt } from "@/lib/ladder";
import { cn } from "@nativelink/ui";

interface RungProps {
  /** Which rung of the ladder this page sits on. */
  n: number;
  /** Total rungs, if a page needs to override the default ladder length. */
  of?: number;
  /**
   * What this page assumes the reader already has. Written as the state they
   * are in, not as the page they read: "a cache serving hits", not "the
   * quickstart".
   */
  requires?: string;
  className?: string;
}

/**
 * States where a page sits on the CAS-to-RBE ladder.
 *
 * Readers arrive mid-corpus from search and from agent caches, not only from
 * the page before. A page in an ordered track therefore has to say so itself:
 * which rung it is, and what it assumes you already have. Without that, a
 * reader who lands on remote execution with no working cache follows correct
 * instructions to a broken result and blames the product.
 */
export function Rung({ n, of = LADDER_LENGTH, requires, className }: RungProps) {
  const here = rungAt(n);
  const previous = n > 1 ? rungAt(n - 1) : undefined;
  const assumes = requires ?? previous?.outcome;

  return (
    <div
      data-rung={n}
      data-rung-of={of}
      className={cn(
        "my-6 rounded-xl border border-border bg-surface-elevated px-5 py-3",
        className,
      )}
    >
      <p className="my-0 font-mono text-xs font-semibold uppercase tracking-[0.12em] text-brand">
        Rung {n} of {of}
        {here ? ` — ${here.title}` : null}
      </p>
      {assumes ? (
        <p className="my-0 mt-1 text-sm text-muted-foreground">
          Assumes you already have {assumes}
          {previous ? (
            <>
              {" — "}
              <a href={previous.href} className="text-brand">
                rung {previous.n}, {previous.title}
              </a>
            </>
          ) : null}
          .
        </p>
      ) : null}
    </div>
  );
}
