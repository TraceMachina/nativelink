// The subpath keeps this data module (and its test) off the full component barrel.
import { isCurrentNavHref } from "@nativelink/ui/nav";

/**
 * The marketing site's canonical calls to action.
 *
 * One label per destination: a reader who has seen a button once can predict
 * where every other copy of it lands (a finding of the UX audit of nativelink.com).
 * Pages compose these constants; they do not invent new labels or variants.
 */
export interface Cta {
  label: string;
  href: string;
  /** Leaves the Next.js app (docs, GitHub), so it opens in a new tab. */
  external?: boolean;
}

/** The primary action everywhere: the docs quickstart. */
export const GET_STARTED: Cta = { label: "Get started", href: "/docs", external: true };

/** The human-contact action everywhere: the Contact page, which shows the addresses. */
export const TALK_TO_US: Cta = { label: "Talk to us", href: "/contact" };

export const SEE_PRICING: Cta = { label: "See pricing", href: "/pricing" };

export const CLONE_THE_REPO: Cta = {
  label: "Clone the repo",
  href: "https://github.com/TraceMachina/nativelink",
  external: true,
};

/**
 * Secondary action for the closing band on every page.
 *
 * The band is shared by the root layout, so without this the Pricing page
 * would close with a "See pricing" button that links to itself.
 */
export function closingSecondaryCta(pathname: string | null | undefined): Cta {
  return isCurrentNavHref(SEE_PRICING.href, pathname ?? undefined) ? TALK_TO_US : SEE_PRICING;
}
