"use client";

import { SiteHeader } from "@nativelink/ui";
import { usePathname } from "next/navigation";

/** Client wrapper so the shared header can mark the current route. */
export function AppSiteHeader() {
  const pathname = usePathname();
  return <SiteHeader currentPath={pathname} />;
}
