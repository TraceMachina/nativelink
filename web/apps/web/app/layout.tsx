import { FinalCTA, SiteFooter, SiteHeader, ThemeProvider, themeInitScript } from "@nativelink/ui";
import { GoogleTagManager } from "@next/third-parties/google";
import { GeistMono } from "geist/font/mono";
import { GeistSans } from "geist/font/sans";
import type { Metadata } from "next";
import Script from "next/script";
import type { ReactNode } from "react";
import "./globals.css";

const gtmId = process.env.NEXT_PUBLIC_GTM_ID;
const lsi = process.env.NEXT_PUBLIC_LSI;
const lsu = process.env.NEXT_PUBLIC_LSU;
const lth = process.env.NEXT_PUBLIC_LTH;

if (process.env.NODE_ENV === "production") {
  for (const [key, value] of Object.entries({
    NEXT_PUBLIC_GTM_ID: gtmId,
    NEXT_PUBLIC_LSI: lsi,
    NEXT_PUBLIC_LSU: lsu,
    NEXT_PUBLIC_LTH: lth,
  })) {
    if (!value) {
      console.warn(`${key} unset — omitted from this build`);
    }
  }
}

const lsScript =
  lsi && lsu
    ? `(function(ss,ex){ window.ldfdr=window.ldfdr||function(){(ldfdr._q=ldfdr._q||[]).push([].slice.call(arguments));}; (function(d,s){ fs=d.getElementsByTagName(s)[0]; function ce(src){ var cs=d.createElement(s); cs.src=src; cs.async=1; fs.parentNode.insertBefore(cs,fs); }; ce('${lsu}'+ss+(ex?'_'+ex:'')+'.js'); })(document,'script'); })('${lsi}');`
    : undefined;

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
    <html
      lang="en"
      suppressHydrationWarning
      className={`${GeistSans.variable} ${GeistMono.variable}`}
    >
      <head>
        {lsu && <link rel="preconnect" href={new URL(lsu).origin} />}
        {lth && <link rel="preconnect" href={lth} />}
        {/* biome-ignore lint/security/noDangerouslySetInnerHtml: Inline before hydration so the theme is correct on first paint. */}
        <script dangerouslySetInnerHTML={{ __html: themeInitScript }} />
      </head>
      {gtmId && <GoogleTagManager gtmId={gtmId} />}
      <body className="flex min-h-screen flex-col bg-background text-foreground antialiased">
        <ThemeProvider>
          <SiteHeader />
          <main id="main" className="flex-1">
            {children}
          </main>
          <FinalCTA
            title="Let's build at the speed your code is being written."
            body="Open source. Free cloud tier. Self-host the moment your team is ready."
            primaryLabel="Get started"
            primaryHref="/docs"
            primaryNewTab
            secondaryLabel="See pricing"
            secondaryHref="/pricing"
          />
          <SiteFooter />
        </ThemeProvider>
        {lsScript && (
          <Script id="ls" strategy="afterInteractive">
            {lsScript}
          </Script>
        )}
      </body>
    </html>
  );
}
