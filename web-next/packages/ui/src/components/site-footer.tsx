import { cn } from "../lib/cn";
import { Logo } from "./logo";

interface FooterColumn {
  title: string;
  links: { label: string; href: string }[];
}

interface SiteFooterProps {
  columns?: FooterColumn[];
  tagline?: string;
  className?: string;
}

const defaultColumns: FooterColumn[] = [
  {
    title: "Product",
    links: [
      { label: "Product", href: "/product" },
      { label: "Pricing", href: "/pricing" },
      { label: "Community", href: "/community" },
      { label: "Docs", href: "/docs" },
    ],
  },
  {
    title: "Company",
    links: [
      { label: "About", href: "/company" },
      { label: "Resources", href: "/resources" },
      { label: "Terms & Privacy", href: "/terms" },
      { label: "Compliance", href: "/compliance" },
      { label: "Contact", href: "mailto:contact@nativelink.com" },
    ],
  },
  {
    title: "Connect",
    links: [
      { label: "GitHub", href: "https://github.com/TraceMachina/nativelink" },
      { label: "Slack", href: "https://forms.gle/LtaWSixEC6bYi5xF7" },
      { label: "Status", href: "/status" },
    ],
  },
];

export function SiteFooter({
  columns = defaultColumns,
  tagline = "High-performance remote build cache and execution. Open source. Self-host or run on our cloud.",
  className,
}: SiteFooterProps) {
  const year = new Date().getFullYear();
  return (
    <footer className={cn("border-t border-border/60 bg-background", className)}>
      <div className="mx-auto grid w-full max-w-[1200px] grid-cols-2 gap-10 px-6 py-16 sm:grid-cols-4 sm:gap-8">
        <div className="col-span-2 flex flex-col gap-4 sm:col-span-1">
          <a href="/" aria-label="NativeLink — home" className="inline-flex">
            <Logo size="md" />
          </a>
          <p className="max-w-[18rem] text-sm leading-relaxed text-muted-foreground">
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
                    className="inline-flex min-h-[36px] items-center text-sm text-muted-foreground transition-colors hover:text-foreground"
                  >
                    {link.label}
                  </a>
                </li>
              ))}
            </ul>
          </nav>
        ))}
      </div>

      <div className="border-t border-border/60">
        <div className="mx-auto flex w-full max-w-[1200px] flex-col items-start justify-between gap-3 px-6 py-6 sm:flex-row sm:items-center">
          <p className="font-mono text-xs text-muted">© Trace Machina {year}</p>
          <p className="flex items-center gap-1.5 font-mono text-xs text-muted">
            <span className="inline-block h-1.5 w-1.5 rounded-full bg-success" />
            All systems operational
          </p>
        </div>
      </div>
    </footer>
  );
}
