import * as React from "react";
import { cn } from "../lib/cn";

export function Prose({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        // Article body: the sans reading font (not mono), muted for calm
        // long-form text. Headings, links and code opt back into stronger
        // colors below. Only code/pre stay monospace.
        "max-w-[68ch] text-[1.0625rem] leading-[1.75] text-muted-foreground",
        // Headings — foreground, tight tracking, generous lead-in space.
        "[&_h1]:mb-6 [&_h1]:text-4xl [&_h1]:font-semibold [&_h1]:leading-[1.15] [&_h1]:tracking-[-0.025em] [&_h1]:text-foreground",
        "[&_h2]:mt-12 [&_h2]:mb-4 [&_h2]:text-[1.6rem] [&_h2]:font-semibold [&_h2]:leading-[1.25] [&_h2]:tracking-[-0.02em] [&_h2]:text-foreground",
        "[&_h3]:mt-9 [&_h3]:mb-3 [&_h3]:text-xl [&_h3]:font-semibold [&_h3]:leading-[1.3] [&_h3]:text-foreground",
        "[&_h4]:mt-8 [&_h4]:mb-2 [&_h4]:text-lg [&_h4]:font-semibold [&_h4]:text-foreground",
        // Paragraphs and inline emphasis.
        "[&_p]:my-[1.15em]",
        "[&_strong]:font-semibold [&_strong]:text-foreground",
        "[&_a]:font-medium [&_a]:text-brand [&_a]:underline [&_a]:underline-offset-4 hover:[&_a]:text-brand-strong",
        // Lists.
        "[&_ul]:my-[1.15em] [&_ul]:list-disc [&_ul]:pl-6",
        "[&_ol]:my-[1.15em] [&_ol]:list-decimal [&_ol]:pl-6",
        "[&_li]:my-1.5 [&_li]:pl-1",
        // Blockquote.
        "[&_blockquote]:my-6 [&_blockquote]:border-l-2 [&_blockquote]:border-brand/50 [&_blockquote]:pl-5 [&_blockquote]:italic [&_blockquote]:text-foreground",
        // Images.
        "[&_img]:my-8 [&_img]:rounded-xl",
        "[&_hr]:my-10 [&_hr]:border-border/60",
        // Code is the only monospace: inline spans and fenced blocks.
        "[&_code]:rounded [&_code]:bg-foreground/5 [&_code]:px-1.5 [&_code]:py-0.5 [&_code]:font-mono [&_code]:text-[0.875em]",
        "[&_pre]:my-6 [&_pre]:overflow-x-auto [&_pre]:rounded-lg [&_pre]:border [&_pre]:border-border [&_pre]:bg-foreground/[0.03] [&_pre]:p-4 [&_pre]:font-mono [&_pre]:text-sm [&_pre]:leading-relaxed",
        "[&_pre_code]:bg-transparent [&_pre_code]:p-0 [&_pre_code]:text-inherit",
        className,
      )}
      {...props}
    />
  );
}
