"use client";

import { GET_STARTED } from "@/lib/cta";
import { SiteHeader } from "@nativelink/ui";
import { usePathname } from "next/navigation";

/** Client wrapper so the shared header can mark the current route and share the canonical CTA. */
export function AppSiteHeader() {
  const pathname = usePathname();
  return (
    <SiteHeader currentPath={pathname} ctaLabel={GET_STARTED.label} ctaHref={GET_STARTED.href} />
  );
}
