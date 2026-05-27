import { RootProvider } from "fumadocs-ui/provider";
import { GeistMono } from "geist/font/mono";
import { GeistSans } from "geist/font/sans";
import type { Metadata } from "next";
import type { ReactNode } from "react";
import { DocsLayout } from "fumadocs-ui/layouts/docs";
import { source } from "@/lib/source";
import { baseOptions } from "./layout.config";
import "./globals.css";

export const metadata: Metadata = {
  title: {
    default: "NativeLink Docs",
    template: "%s — NativeLink Docs",
  },
  description:
    "Documentation for NativeLink — high-performance remote build cache & execution.",
  metadataBase: new URL("https://nativelink.com"),
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html
      lang="en"
      suppressHydrationWarning
      className={`${GeistSans.variable} ${GeistMono.variable}`}
    >
      <body className="flex min-h-screen flex-col bg-background text-foreground antialiased">
        {/* RootProvider configures the theme via next-themes. We point it
         * at our data-theme attribute (matching marketing) and share the
         * same localStorage key so the theme persists across both apps. */}
        <RootProvider
          theme={{
            attribute: "data-theme",
            defaultTheme: "dark",
            enableSystem: true,
            storageKey: "nl-theme",
          }}
          search={{ options: { api: "/docs/api/search" } }}
        >
          <DocsLayout tree={source.pageTree} {...baseOptions}>
            {children}
          </DocsLayout>
        </RootProvider>
      </body>
    </html>
  );
}
