import { Button, Eyebrow, Reveal, Section, cn } from "@nativelink/ui";
import { Fragment } from "react";

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
      "All build systems (Bazel, Buck2, Siso, Pants)",
      "All major cloud providers",
    ],
    cta: { label: "Get started", href: "/docs" },
  },
  {
    name: "Enterprise",
    price: "Custom",
    cadence: "annually",
    tagline: "On-prem deployment with dedicated engineering.",
    features: [
      "Single-tenant on-prem deployment",
      "Dedicated solutions engineer",
      "Custom SLAs & security review",
      "Autoscaling, GUI, build breakdown",
      "Procurement support",
      "Priority feature requests",
    ],
    cta: { label: "Subscribe now", href: "https://enterprise.nativelink.com" },
  },
];

const comparison: {
  section: string;
  rows: { label: string; oss: string | boolean; ent: string | boolean }[];
}[] = [
  {
    section: "Platform",
    rows: [
      { label: "Hosting", oss: "Self-hosted", ent: "On-prem or managed" },
      { label: "Distributed scheduler", oss: true, ent: true },
      { label: "Remote caching", oss: true, ent: true },
      { label: "Remote execution", oss: true, ent: true },
      { label: "Cross-compilation", oss: true, ent: true },
      { label: "External storage (S3, Redis)", oss: true, ent: true },
      { label: "Autoscaling", oss: false, ent: true },
      { label: "Multi-region", oss: "DIY", ent: true },
    ],
  },
  {
    section: "Compatibility",
    rows: [
      { label: "Supported build systems", oss: "All", ent: "All + custom" },
      {
        label: "Operating systems",
        oss: "Linux, macOS",
        ent: "Linux, macOS, Windows",
      },
      { label: "Org-wide sharing", oss: true, ent: true },
    ],
  },
  {
    section: "Operations",
    rows: [
      { label: "GUI dashboard", oss: false, ent: true },
      { label: "Build action breakdown", oss: false, ent: true },
      { label: "Live build updates", oss: false, ent: true },
      { label: "Audit logs / SSO", oss: false, ent: true },
    ],
  },
  {
    section: "Support",
    rows: [
      {
        label: "Channel",
        oss: "Community Slack",
        ent: "Dedicated engineer",
      },
      { label: "Onboarding", oss: false, ent: "White-glove" },
    ],
  },
];

function Cell({ value }: { value: string | boolean }) {
  if (typeof value === "boolean") {
    return (
      <span
        aria-label={value ? "Included" : "Not included"}
        className={cn(
          "inline-flex h-5 w-5 items-center justify-center rounded-full font-mono text-[11px]",
          value ? "bg-brand-soft text-brand" : "bg-foreground/[0.04] text-muted",
        )}
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
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[500px] bg-[radial-gradient(ellipse_900px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.15),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-10 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[820px] text-center">
              <Eyebrow className="mb-5">Pricing</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] md:text-[68px]">
                Built to{" "}
                <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
                  scale with your infrastructure
                </span>
                .
              </h1>
              <p className="mx-auto mt-6 max-w-[640px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                Self-host for free, or let us run your build farm. Two tiers that grow with your
                team — no hidden fees, no per-user pricing.
              </p>
            </div>
          </Reveal>
        </Section>
      </section>

      {/* TIERS */}
      <Section width="default" className="pb-24">
        <div className="mx-auto grid max-w-[760px] gap-5 md:grid-cols-2">
          {tiers.map((tier, i) => (
            <Reveal key={tier.name} delay={i * 0.06}>
              <div className="relative flex h-full flex-col rounded-2xl border border-border bg-surface p-8">
                <h3 className="font-mono text-xs uppercase tracking-[0.18em] text-muted">
                  {tier.name}
                </h3>
                <div className="mt-5 flex items-baseline gap-2">
                  <span className="text-[44px] font-semibold leading-none tracking-tight text-foreground">
                    {tier.price}
                  </span>
                  <span className="font-mono text-sm text-muted">{tier.cadence}</span>
                </div>
                <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                  {tier.tagline}
                </p>
                <ul className="mt-8 flex flex-col gap-3 text-sm">
                  {tier.features.map((f) => (
                    <li key={f} className="flex items-start gap-3">
                      <span
                        aria-hidden="true"
                        className="mt-0.5 inline-flex h-4 w-4 items-center justify-center rounded-full bg-brand-soft text-[10px] text-brand"
                      >
                        ✓
                      </span>
                      <span className="text-foreground">{f}</span>
                    </li>
                  ))}
                </ul>
                <div className="mt-auto pt-10">
                  <Button asChild size="lg" variant="outline" className="w-full">
                    <a
                      href={tier.cta.href}
                      target={tier.cta.label === "Get started" ? "_blank" : undefined}
                      rel={tier.cta.label === "Get started" ? "noreferrer" : undefined}
                    >
                      {tier.cta.label}
                    </a>
                  </Button>
                </div>
              </div>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* COMPARISON */}
      <Section width="default" className="py-28">
        <Reveal>
          <div className="mx-auto mb-12 max-w-[680px] text-center">
            <Eyebrow className="mb-4">Compare</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Feature-for-feature comparison
            </h2>
          </div>
        </Reveal>

        <Reveal>
          <div className="mx-auto max-w-[980px] overflow-hidden rounded-2xl border border-border bg-surface">
            <table className="w-full border-collapse text-left">
              <thead>
                <tr>
                  <th className="w-[40%] bg-surface-elevated px-6 py-4 font-mono text-[10px] uppercase tracking-widest text-muted">
                    Feature
                  </th>
                  <th className="bg-surface-elevated px-6 py-4 font-mono text-[10px] uppercase tracking-widest text-muted">
                    Open Source
                  </th>
                  <th className="bg-surface-elevated px-6 py-4 font-mono text-[10px] uppercase tracking-widest text-muted">
                    Enterprise
                  </th>
                </tr>
              </thead>
              <tbody>
                {comparison.map((group) => (
                  <Fragment key={group.section}>
                    <tr>
                      <td
                        colSpan={3}
                        className="border-t border-border bg-foreground/[0.015] px-6 py-3 font-mono text-[10px] uppercase tracking-[0.18em] text-muted-foreground"
                      >
                        {group.section}
                      </td>
                    </tr>
                    {group.rows.map((row) => (
                      <tr key={row.label} className="border-t border-border">
                        <td className="px-6 py-4 text-[15px] text-foreground">{row.label}</td>
                        <td className="px-6 py-4">
                          <Cell value={row.oss} />
                        </td>
                        <td className="px-6 py-4">
                          {row.label === "Hosting" ? (
                            <a
                              href="https://enterprise.nativelink.com"
                              target="_blank"
                              rel="noreferrer"
                              className="font-mono text-sm text-brand underline-offset-4 hover:underline"
                            >
                              {row.ent}
                            </a>
                          ) : (
                            <Cell value={row.ent} />
                          )}
                        </td>
                      </tr>
                    ))}
                  </Fragment>
                ))}
              </tbody>
            </table>
          </div>
        </Reveal>
      </Section>

      {/* BOTTOM CONTACT */}
      <Section width="narrow" className="border-t border-border/60 py-24 text-center">
        <Reveal>
          <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
            Have a question?
          </h2>
          <p className="mx-auto mt-5 max-w-[560px] text-base leading-relaxed text-muted-foreground md:text-lg">
            Our team is happy to walk through pricing for your team or workload.
          </p>
          <div className="mt-8 flex flex-wrap justify-center gap-3">
            <Button size="lg" asChild>
              <a href="mailto:contact@tracemachina.com">Talk to sales</a>
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
