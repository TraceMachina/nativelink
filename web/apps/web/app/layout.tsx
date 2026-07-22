import { FinalCTA, SiteFooter, SiteHeader, ThemeProvider, themeInitScript } from "@nativelink/ui";
import { GeistMono } from "geist/font/mono";
import { GeistSans } from "geist/font/sans";
import type { Metadata } from "next";
import Script from "next/script";
import type { ReactNode } from "react";
import "./globals.css";

// Official Leadfeeder (Dealfront) tracker snippet, verbatim. The site ID is the
// same one deployed on tracemachina.com.
const leadfeederScript = `(function(ss,ex){ window.ldfdr=window.ldfdr||function(){(ldfdr._q=ldfdr._q||[]).push([].slice.call(arguments));}; (function(d,s){ fs=d.getElementsByTagName(s)[0]; function ce(src){ var cs=d.createElement(s); cs.src=src; cs.async=1; fs.parentNode.insertBefore(cs,fs); }; ce('https://sc.lfeeder.com/lftracker_v1_'+ss+(ex?'_'+ex:'')+'.js'); })(document,'script'); })('lAxoEaKMQGd7OYGd');`;

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
        {/* Leadfeeder origins: script host and beacon host. No crossorigin —
            the tracker loads as a plain script, which can't reuse a CORS-warmed
            connection. */}
        <link rel="preconnect" href="https://sc.lfeeder.com" />
        <link rel="preconnect" href="https://tr.lfeeder.com" />
        {/* biome-ignore lint/security/noDangerouslySetInnerHtml: Inline before hydration so the theme is correct on first paint. */}
        <script dangerouslySetInnerHTML={{ __html: themeInitScript }} />
      </head>
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
        <Script id="leadfeeder" strategy="afterInteractive">
          {leadfeederScript}
        </Script>
      </body>
    </html>
  );
}
