import type { Metadata } from "next";
import type { ReactNode } from "react";
import "./globals.css";

export const metadata: Metadata = {
  title: {
    default: "NativeLink",
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
      <body>{children}</body>
    </html>
  );
}
