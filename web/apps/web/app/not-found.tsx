import { Button, Eyebrow, Section } from "@nativelink/ui";

export const metadata = {
  title: "Page not found",
  robots: { index: false, follow: false },
};

const links = [
  { label: "Home", href: "/", description: "Start at the top." },
  { label: "Product", href: "/product", description: "What NativeLink does." },
  { label: "Docs", href: "/docs", description: "Setup and reference." },
  { label: "Contact", href: "/contact", description: "Talk to the team." },
];

export default function NotFound() {
  return (
    <section className="relative overflow-hidden">
      <div className="pointer-events-none absolute inset-0 -z-10 bg-brand-glow" />
      <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[600px] bg-[radial-gradient(ellipse_900px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.16),transparent_70%)]" />

      <Section width="narrow" className="pt-24 pb-32 text-center md:pt-32">
        <Eyebrow className="mb-5">404</Eyebrow>
        <h1 className="text-balance text-[64px] font-semibold leading-[1.02] tracking-[-0.04em] md:text-[96px]">
          <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
            Page not found
          </span>
        </h1>
        <p className="mx-auto mt-6 max-w-[520px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
          The page you're looking for doesn't exist, was moved, or maybe never shipped. Here's where
          you might want to go instead.
        </p>

        <div className="mt-8 flex flex-wrap justify-center gap-3">
          <Button size="lg" asChild>
            <a href="/">Back to home</a>
          </Button>
          <Button size="lg" variant="outline" asChild>
            <a href="/docs">Read the docs</a>
          </Button>
        </div>

        <div className="mt-16 grid gap-3 sm:grid-cols-2">
          {links.map((l) => (
            <a
              key={l.href}
              href={l.href}
              className="group flex items-center justify-between gap-4 rounded-2xl border border-border bg-surface px-6 py-4 text-left transition-all hover:-translate-y-0.5 hover:border-brand/40"
            >
              <div>
                <p className="font-mono text-sm font-semibold text-foreground">{l.label}</p>
                <p className="mt-0.5 text-sm text-muted-foreground">{l.description}</p>
              </div>
              <span
                aria-hidden="true"
                className="text-muted transition-all group-hover:translate-x-1 group-hover:text-brand"
              >
                →
              </span>
            </a>
          ))}
        </div>
      </Section>
    </section>
  );
}
