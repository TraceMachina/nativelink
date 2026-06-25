import { Section } from "@nativelink/ui";
import Link from "next/link";
import type { ReactNode } from "react";

export default function BlogLayout({ children }: { children: ReactNode }) {
  return (
    <Section width="default" className="pt-28 pb-24 md:pt-32">
      <div className="mx-auto max-w-[760px]">
        <Link
          href="/resources"
          className="mb-10 inline-flex items-center gap-1.5 font-mono text-sm text-muted-foreground transition-colors hover:text-brand"
        >
          <span aria-hidden="true">←</span> Resources
        </Link>
        <article className="prose-styles">{children}</article>
        <div className="mt-16 border-t border-border/60 pt-8">
          <Link
            href="/resources"
            className="inline-flex items-center gap-1.5 font-mono text-sm text-brand transition-colors hover:text-brand-strong"
          >
            <span aria-hidden="true">←</span> Back to all resources
          </Link>
        </div>
      </div>
    </Section>
  );
}
