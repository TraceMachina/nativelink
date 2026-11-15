"use client";

import { GET_STARTED, closingSecondaryCta } from "@/lib/cta";
import { FinalCTA } from "@nativelink/ui";
import { usePathname } from "next/navigation";

/** Client wrapper so the shared closing band never links to the page it is on. */
export function AppFinalCta() {
  const secondary = closingSecondaryCta(usePathname());
  return (
    <FinalCTA
      title="Let's build at the speed your code is being written."
      body="Open source. Self-host the moment your team is ready."
      primaryLabel={GET_STARTED.label}
      primaryHref={GET_STARTED.href}
      primaryNewTab={GET_STARTED.external}
      secondaryLabel={secondary.label}
      secondaryHref={secondary.href}
      secondaryNewTab={secondary.external}
    />
  );
}
