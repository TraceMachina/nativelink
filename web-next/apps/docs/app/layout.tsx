import { RootProvider } from "fumadocs-ui/provider";
import type { Metadata } from "next";
import type { ReactNode } from "react";
import "./globals.css";

export const metadata: Metadata = {
  title: {
    default: "NativeLink Docs",
    template: "%s — NativeLink Docs",
  },
  description: "Documentation for NativeLink — high-performance remote build cache & execution.",
  metadataBase: new URL("https://nativelink.com"),
};

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body>
        <RootProvider>{children}</RootProvider>
      </body>
    </html>
  );
}
