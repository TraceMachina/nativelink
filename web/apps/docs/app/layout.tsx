import { source } from "@/lib/source";
import { DocsLayout } from "fumadocs-ui/layouts/docs";
import { RootProvider } from "fumadocs-ui/provider/next";
import { GeistMono } from "geist/font/mono";
import { GeistSans } from "geist/font/sans";
import type { Metadata } from "next";
import type { ReactNode } from "react";
import { baseOptions } from "./layout.config";
import "./globals.css";

export const metadata: Metadata = {
  title: {
    default: "NativeLink Docs",
    template: "%s — NativeLink Docs",
  },
  description: "Documentation for NativeLink — high-performance remote build cache & execution.",
  metadataBase: new URL("https://docs.nativelink.com"),
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html
      lang="en"
      suppressHydrationWarning
      className={`${GeistSans.variable} ${GeistMono.variable}`}
    >
      <head>
        <link rel="llms-txt" href="/llms.txt" />
      </head>
      <body className="flex min-h-screen flex-col bg-background text-foreground antialiased">
        {/* RootProvider configures the theme via next-themes. Setting BOTH
         * "class" and "data-theme" attributes is intentional:
         *   - .dark class drives Fumadocs's built-in Shiki dual-theme code
         *     blocks (the CSS keys off html.dark, not data-theme)
         *   - data-theme="dark" drives our @nativelink/tokens brand vars
         *     used everywhere else in the UI.
         * Same localStorage key as marketing so the toggle persists across
         * both apps. */}
        <RootProvider
          theme={{
            attribute: ["class", "data-theme"],
            defaultTheme: "dark",
            enableSystem: true,
            storageKey: "nl-theme",
          }}
          search={{ options: { api: "/api/search" } }}
        >
          <DocsLayout tree={source.pageTree} {...baseOptions}>
            {children}
          </DocsLayout>
        </RootProvider>
      </body>
    </html>
  );
}
