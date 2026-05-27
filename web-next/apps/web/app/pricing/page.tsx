import { Badge, Button, Eyebrow, Reveal, Section, cn } from "@nativelink/ui";

export const metadata = { title: "Pricing" };

const tiers = [
  {
    name: "Open Source",
    price: "Free",
    cadence: "forever",
    tagline: "Everything you need to self-host NativeLink.",
    features: [
      "Self-hosted",
      "Community support on Slack",
      "Distributed scheduler & remote caching",
      "All build systems (Bazel, Buck2, Reclient, Pants)",
    ],
    cta: { label: "Get started", href: "/docs", variant: "outline" as const },
    featured: false,
  },
  {
    name: "Cloud",
    price: "$1,000",
    cadence: "per month",
    tagline: "Managed NativeLink. We run the infra, you ship.",
    features: [
      "Fully managed cluster",
      "Automatic scaling & failover",
      "Production SLAs",
      "Onboarding support",
    ],
    cta: { label: "Start free trial", href: "https://dev.nativelink.com/", variant: "primary" as const },
    featured: true,
  },
  {
    name: "Enterprise",
    price: "Custom",
    cadence: "billed annually",
    tagline: "On-prem deployment with dedicated engineering.",
    features: [
      "Single-tenant on-prem deployment",
      "Dedicated engineer",
      "Custom SLAs & security review",
      "Autoscaling, GUI, build breakdown",
    ],
    cta: { label: "Request quote", href: "mailto:hello@nativelink.com", variant: "outline" as const },
    featured: false,
  },
];

const comparison: { label: string; oss: string | boolean; ent: string | boolean }[] = [
  { label: "Hosting", oss: "Self-hosted", ent: "Self-hosted or managed" },
  { label: "Support", oss: "Community", ent: "Dedicated engineer" },
  {
    label: "Supported build systems",
    oss: "Bazel · Buck2 · Reclient · Pants",
    ent: "Bazel · Buck2 · Reclient · Pants",
  },
  { label: "Operating systems", oss: "Linux, macOS", ent: "Linux, macOS, Windows" },
  { label: "Org-wide sharing", oss: true, ent: true },
  { label: "Distributed scheduler", oss: true, ent: true },
  { label: "Remote caching", oss: true, ent: true },
  { label: "Cross-compilation", oss: true, ent: true },
  { label: "External storage (S3, Redis)", oss: true, ent: true },
  { label: "Remote execution", oss: true, ent: true },
  { label: "Autoscaling", oss: false, ent: true },
  { label: "GUI dashboard", oss: false, ent: true },
  { label: "Build action breakdown", oss: false, ent: true },
  { label: "Live build updates", oss: false, ent: true },
];

function CheckCell({ value }: { value: string | boolean }) {
  if (typeof value === "boolean") {
    return (
      <span
        aria-label={value ? "Included" : "Not included"}
        className={cn("inline-block font-mono text-base", !value && "text-muted")}
      >
        {value ? "✓" : "—"}
      </span>
    );
  }
  return <span className="font-mono text-sm text-foreground">{value}</span>;
}

export default function PricingPage() {
  return (
    <>
      {/* HERO */}
      <Section width="default" className="pt-24 pb-12 md:pt-32">
        <Reveal>
          <div className="mx-auto max-w-[820px] text-center">
            <Eyebrow className="mb-5">Pricing</Eyebrow>
            <h1 className="text-balance text-4xl font-bold leading-[1.1] tracking-tight md:text-6xl">
              Built to scale with your infrastructure.
            </h1>
            <p className="mx-auto mt-6 max-w-[600px] text-lg leading-relaxed text-muted">
              Start free, self-host, or let us run your build farm. Three tiers that
              grow with your team — no hidden fees, no per-user pricing.
            </p>
          </div>
        </Reveal>
      </Section>

      {/* TIERS */}
      <Section width="default" className="pb-24">
        <div className="grid gap-6 md:grid-cols-3">
          {tiers.map((tier, i) => (
            <Reveal key={tier.name} delay={i * 0.05}>
              <div
                className={cn(
                  "relative flex h-full flex-col rounded-md border-2 p-8",
                  tier.featured
                    ? "border-foreground bg-foreground text-background"
                    : "border-border bg-surface/50",
                )}
              >
                {tier.featured && (
                  <Badge
                    variant="outline"
                    className="absolute -top-3 left-8 border-foreground bg-background text-foreground"
                  >
                    Recommended
                  </Badge>
                )}
                <h3
                  className={cn(
                    "font-mono text-sm uppercase tracking-widest",
                    tier.featured ? "text-background/70" : "text-muted",
                  )}
                >
                  {tier.name}
                </h3>
                <div className="mt-4 flex items-baseline gap-2">
                  <span className="text-4xl font-bold leading-none md:text-5xl">
                    {tier.price}
                  </span>
                  <span
                    className={cn(
                      "font-mono text-sm",
                      tier.featured ? "text-background/70" : "text-muted",
                    )}
                  >
                    {tier.cadence}
                  </span>
                </div>
                <p
                  className={cn(
                    "mt-3 text-base leading-relaxed",
                    tier.featured ? "text-background/80" : "text-muted-foreground",
                  )}
                >
                  {tier.tagline}
                </p>
                <ul
                  className={cn(
                    "mt-8 flex flex-col gap-3 text-sm",
                    tier.featured ? "text-background" : "text-foreground",
                  )}
                >
                  {tier.features.map((f) => (
                    <li key={f} className="flex items-start gap-3">
                      <span
                        aria-hidden="true"
                        className={cn(
                          "mt-1 font-mono text-xs",
                          tier.featured ? "text-background/70" : "text-muted",
                        )}
                      >
                        ✓
                      </span>
                      {f}
                    </li>
                  ))}
                </ul>
                <div className="mt-auto pt-10">
                  <Button
                    asChild
                    size="lg"
                    variant={
                      tier.featured ? "outline" : tier.cta.variant === "primary" ? "primary" : "outline"
                    }
                    className={cn(
                      "w-full",
                      tier.featured && "border-background bg-background text-foreground hover:bg-background/90",
                    )}
                  >
                    <a href={tier.cta.href}>{tier.cta.label}</a>
                  </Button>
                </div>
              </div>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* COMPARISON */}
      <Section
        width="default"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-24"
      >
        <Reveal>
          <div className="mx-auto mb-12 max-w-[680px] text-center">
            <Eyebrow className="mb-4">Compare</Eyebrow>
            <h2 className="text-3xl font-bold leading-[1.15] md:text-5xl">
              Feature-for-feature comparison
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted md:text-lg">
              Everything that ships with each tier, side-by-side. Open Source and
              Enterprise share the same engine; Enterprise adds operational tooling on
              top.
            </p>
          </div>
        </Reveal>
        <Reveal>
          <div className="mx-auto max-w-[920px] overflow-hidden rounded-md border-2 border-border">
            <table className="w-full border-collapse text-left text-sm">
              <thead>
                <tr className="border-b-2 border-border bg-surface/50">
                  <th className="px-5 py-4 font-mono text-xs uppercase tracking-widest text-muted">
                    Feature
                  </th>
                  <th className="px-5 py-4 font-mono text-xs uppercase tracking-widest text-muted">
                    Open Source
                  </th>
                  <th className="px-5 py-4 font-mono text-xs uppercase tracking-widest text-muted">
                    Enterprise
                  </th>
                </tr>
              </thead>
              <tbody>
                {comparison.map((row, i) => (
                  <tr
                    key={row.label}
                    className={cn(
                      "border-b border-border last:border-b-0",
                      i % 2 === 1 && "bg-foreground/[0.02]",
                    )}
                  >
                    <td className="px-5 py-4 font-medium text-foreground">{row.label}</td>
                    <td className="px-5 py-4">
                      <CheckCell value={row.oss} />
                    </td>
                    <td className="px-5 py-4">
                      <CheckCell value={row.ent} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Reveal>
      </Section>

      {/* CONTACT */}
      <Section
        width="narrow"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-24 text-center"
      >
        <Reveal>
          <h2 className="text-balance text-3xl font-bold leading-[1.15] md:text-5xl">
            Have a question?
          </h2>
          <p className="mx-auto mt-5 max-w-[560px] text-base leading-relaxed text-muted md:text-lg">
            Our team is happy to walk through pricing for your team or workload.
          </p>
          <div className="mt-8 flex flex-wrap justify-center gap-3">
            <Button size="lg" asChild>
              <a href="mailto:hello@nativelink.com">Talk to sales</a>
            </Button>
            <Button size="lg" variant="outline" asChild>
              <a href="/community">Join community</a>
            </Button>
          </div>
        </Reveal>
      </Section>
    </>
  );
}
