import { cn } from "../lib/cn";
import { Button } from "./button";
import { Logo } from "./logo";
import { MobileNav } from "./mobile-nav";

export interface NavLink {
  label: string;
  href: string;
}

interface SiteHeaderProps {
  links?: NavLink[];
  ctaLabel?: string;
  ctaHref?: string;
  githubHref?: string;
  className?: string;
}

const defaultLinks: NavLink[] = [
  { label: "Product", href: "/product" },
  { label: "Pricing", href: "/pricing" },
  { label: "Company", href: "/company" },
  { label: "Resources", href: "/resources" },
  { label: "Docs", href: "/docs" },
];

export function SiteHeader({
  links = defaultLinks,
  ctaLabel = "Get started",
  ctaHref = "/docs",
  githubHref = "https://github.com/TraceMachina/nativelink",
  className,
}: SiteHeaderProps) {
  return (
    <header
      className={cn(
        "sticky top-0 z-30 w-full",
        "bg-background/80 backdrop-blur supports-[backdrop-filter]:bg-background/60",
        "border-b border-[rgb(var(--nl-color-accent-line))]/40",
        className,
      )}
    >
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:absolute focus:left-4 focus:top-4 focus:z-50 focus:rounded-md focus:bg-foreground focus:px-3 focus:py-2 focus:text-background"
      >
        Skip to content
      </a>

      <div className="mx-auto grid h-16 w-full max-w-[1200px] grid-cols-[1fr_auto_1fr] items-center gap-4 px-6">
        <a
          href="/"
          aria-label="NativeLink — home"
          className="inline-flex items-center justify-self-start"
        >
          <Logo size="md" />
        </a>

        <nav aria-label="Primary" className="hidden md:block">
          <ul className="flex items-center gap-1">
            {links.map((link) => (
              <li key={link.href}>
                <a
                  href={link.href}
                  className="inline-flex h-11 items-center rounded-md px-3 font-mono text-sm text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground"
                >
                  {link.label}
                </a>
              </li>
            ))}
          </ul>
        </nav>

        <div className="flex items-center justify-self-end gap-2">
          <a
            href={githubHref}
            target="_blank"
            rel="noreferrer"
            aria-label="GitHub repository"
            className="hidden h-10 w-10 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground sm:inline-flex"
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.56v-2.01c-3.2.7-3.87-1.36-3.87-1.36-.52-1.34-1.28-1.69-1.28-1.69-1.05-.71.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.76 2.7 1.25 3.36.96.1-.74.4-1.25.73-1.54-2.55-.29-5.23-1.28-5.23-5.68 0-1.25.45-2.28 1.19-3.08-.12-.29-.51-1.46.11-3.04 0 0 .97-.31 3.18 1.18.92-.26 1.91-.39 2.89-.39s1.97.13 2.89.39c2.21-1.49 3.18-1.18 3.18-1.18.62 1.58.23 2.75.11 3.04.74.8 1.19 1.83 1.19 3.08 0 4.41-2.69 5.39-5.25 5.67.41.36.78 1.06.78 2.14v3.17c0 .31.21.68.8.56 4.57-1.52 7.86-5.83 7.86-10.91C23.5 5.65 18.35.5 12 .5Z" />
            </svg>
          </a>

          <Button asChild size="sm" className="hidden md:inline-flex">
            <a href={ctaHref}>{ctaLabel}</a>
          </Button>

          <MobileNav links={links} ctaLabel={ctaLabel} ctaHref={ctaHref} />
        </div>
      </div>
    </header>
  );
}
