import { FinalCTA, SiteFooter, SiteHeader } from "@nativelink/ui";
import type { Metadata } from "next";
import type { ReactNode } from "react";
import "./globals.css";

export const metadata: Metadata = {
  title: {
    default: "NativeLink — Remote build execution & caching",
    template: "%s — NativeLink",
  },
  description:
    "When agents write your code, your build system is the bottleneck. " +
    "NativeLink is the high-performance remote build cache and execution platform for Bazel and beyond.",
  metadataBase: new URL("https://nativelink.com"),
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body className="flex min-h-screen flex-col">
        <SiteHeader />
        <main id="main" className="flex-1">
          {children}
        </main>
        <FinalCTA
          title="Let's build at the speed your code is being written."
          body="Open source. Free cloud tier. Self-host the moment your team is ready."
          primaryLabel="Get started"
          primaryHref="/docs"
          secondaryLabel="See pricing"
          secondaryHref="/pricing"
        />
        <SiteFooter />
      </body>
    </html>
  );
}
