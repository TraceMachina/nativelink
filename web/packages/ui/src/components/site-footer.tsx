import { cn } from "../lib/cn";
import { Logo } from "./logo";
import type { NewsletterState } from "./newsletter-form";
import { NewsletterForm } from "./newsletter-form";

interface FooterColumn {
  title: string;
  links: { label: string; href: string }[];
}

interface SiteFooterProps {
  columns?: FooterColumn[];
  tagline?: string;
  className?: string;
  newsletterAction?: (
    prev: NewsletterState | null,
    formData: FormData,
  ) => Promise<NewsletterState> | NewsletterState;
}

const defaultColumns: FooterColumn[] = [
  {
    title: "Product",
    links: [
      { label: "Product", href: "/product" },
      { label: "Pricing", href: "/pricing" },
      { label: "Docs", href: "/docs" },
      { label: "Enterprise", href: "https://enterprise.nativelink.com" },
      { label: "Status", href: "/status" },
    ],
  },
  {
    title: "Company",
    links: [
      { label: "About", href: "/company" },
      { label: "Community", href: "/community" },
      { label: "Resources", href: "/resources" },
      { label: "Contact", href: "/contact" },
    ],
  },
  {
    title: "Legal",
    links: [
      { label: "Terms & Privacy", href: "/terms" },
      { label: "Compliance", href: "/compliance" },
      { label: "Security", href: "mailto:security@nativelink.com" },
    ],
  },
];

const socialLinks = [
  {
    label: "GitHub",
    href: "https://github.com/TraceMachina/nativelink",
    icon: (
      <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.56v-2.01c-3.2.7-3.87-1.36-3.87-1.36-.52-1.34-1.28-1.69-1.28-1.69-1.05-.71.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.76 2.7 1.25 3.36.96.1-.74.4-1.25.73-1.54-2.55-.29-5.23-1.28-5.23-5.68 0-1.25.45-2.28 1.19-3.08-.12-.29-.51-1.46.11-3.04 0 0 .97-.31 3.18 1.18.92-.26 1.91-.39 2.89-.39s1.97.13 2.89.39c2.21-1.49 3.18-1.18 3.18-1.18.62 1.58.23 2.75.11 3.04.74.8 1.19 1.83 1.19 3.08 0 4.41-2.69 5.39-5.25 5.67.41.36.78 1.06.78 2.14v3.17c0 .31.21.68.8.56 4.57-1.52 7.86-5.83 7.86-10.91C23.5 5.65 18.35.5 12 .5Z" />
    ),
    fill: true,
  },
  {
    label: "Slack",
    href: "https://forms.gle/LtaWSixEC6bYi5xF7",
    icon: (
      <>
        <rect x="3" y="11" width="6" height="2" rx="1" />
        <rect x="11" y="15" width="2" height="6" rx="1" />
        <rect x="15" y="11" width="6" height="2" rx="1" />
        <rect x="11" y="3" width="2" height="6" rx="1" />
        <path d="M9 9h2v2H9zM13 13h2v2h-2z" />
      </>
    ),
    fill: true,
  },
  {
    label: "Email",
    href: "mailto:contact@nativelink.com",
    icon: (
      <path
        d="M3 7 L12 13 L21 7 M3 7 v10 a1 1 0 0 0 1 1 h16 a1 1 0 0 0 1 -1 V7 a1 1 0 0 0 -1 -1 H4 a1 1 0 0 0 -1 1 Z"
        strokeLinejoin="round"
      />
    ),
    fill: false,
  },
];

export function SiteFooter({
  columns = defaultColumns,
  tagline = "High-performance remote build cache and execution. Open source. Self-host or run on our cloud.",
  className,
  newsletterAction,
}: SiteFooterProps) {
  const year = new Date().getFullYear();
  return (
    <footer className={cn("border-t border-border/60 bg-background", className)}>
      <div className="mx-auto grid w-full max-w-[1200px] grid-cols-2 gap-x-8 gap-y-12 px-6 py-16 sm:grid-cols-3 lg:grid-cols-[1.5fr_repeat(3,1fr)_1.4fr]">
        <div className="col-span-2 flex flex-col gap-4 sm:col-span-3 lg:col-span-1">
          <a href="/" aria-label="NativeLink — home" className="inline-flex">
            <Logo size="md" />
          </a>
          <p className="max-w-[20rem] text-sm leading-relaxed text-muted-foreground">
            {tagline}
          </p>
        </div>

        {columns.map((col) => (
          <nav key={col.title} aria-label={col.title}>
            <p className="mb-4 font-mono text-[11px] uppercase tracking-[0.18em] text-muted">
              {col.title}
            </p>
            <ul className="flex flex-col gap-1">
              {col.links.map((link) => (
                <li key={link.href}>
                  <a
                    href={link.href}
                    target={link.href.startsWith("http") ? "_blank" : undefined}
                    rel={link.href.startsWith("http") ? "noreferrer" : undefined}
                    className="inline-flex min-h-[36px] items-center text-sm text-muted-foreground transition-colors hover:text-foreground"
                  >
                    {link.label}
                  </a>
                </li>
              ))}
            </ul>
          </nav>
        ))}

        {newsletterAction ? (
          <div className="col-span-2 sm:col-span-3 lg:col-span-1">
            <p className="mb-4 font-mono text-[11px] uppercase tracking-[0.18em] text-muted">
              Newsletter
            </p>
            <p className="mb-4 text-sm leading-relaxed text-muted-foreground">
              Build performance, deep-tech write-ups. Occasionally.
            </p>
            <NewsletterForm action={newsletterAction} />
          </div>
        ) : null}
      </div>

      <div className="border-t border-border/60">
        <div className="mx-auto flex w-full max-w-[1200px] flex-col items-start justify-between gap-4 px-6 py-6 sm:flex-row sm:items-center">
          <p className="font-mono text-xs text-muted">© Trace Machina {year}</p>

          <div className="flex items-center gap-1">
            {socialLinks.map((s) => (
              <a
                key={s.label}
                href={s.href}
                target={s.href.startsWith("http") ? "_blank" : undefined}
                rel={s.href.startsWith("http") ? "noreferrer" : undefined}
                aria-label={s.label}
                className="inline-flex h-9 w-9 cursor-pointer items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground"
              >
                <svg
                  width="16"
                  height="16"
                  viewBox="0 0 24 24"
                  fill={s.fill ? "currentColor" : "none"}
                  stroke={s.fill ? "none" : "currentColor"}
                  strokeWidth="1.6"
                  aria-hidden="true"
                >
                  {s.icon}
                </svg>
              </a>
            ))}
          </div>

          <a
            href="/status"
            className="flex items-center gap-1.5 font-mono text-xs text-muted transition-colors hover:text-foreground"
          >
            <span className="relative flex h-2 w-2">
              <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-success opacity-60" />
              <span className="relative inline-flex h-2 w-2 rounded-full bg-success" />
            </span>
            All systems operational
          </a>
        </div>
      </div>
    </footer>
  );
}
