import {
  type ComparisonValue,
  ENTERPRISE_URL,
  type TierKey,
  collapseUniversalRows,
  comparison,
} from "@/lib/pricing";
import { Button, Eyebrow, Reveal, Section, cn } from "@nativelink/ui";
import { Fragment } from "react";

export const metadata = {
  title: "Pricing",
  description:
    "Self-host NativeLink for free, or talk to us about Enterprise. Pricing is a conversation: email contact@tracemachina.com.",
};

const PRICING_EMAIL = "contact@tracemachina.com";
const PRICING_MAILTO = `mailto:${PRICING_EMAIL}?subject=${encodeURIComponent("NativeLink pricing")}`;

interface Tier {
  /** Which comparison column this tier owns. */
  key: TierKey;
  name: string;
  price: string;
  cadence: string;
  tagline: string;
  features: string[];
  cta: { label: string; href: string; external?: boolean };
  secondary?: { label: string; href: string };
}

const tiers: Tier[] = [
  {
    key: "oss",
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
    cta: { label: "Get started", href: "/docs", external: true },
  },
  {
    key: "ent",
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
    cta: { label: "Email us about pricing", href: PRICING_MAILTO },
    secondary: {
      label: "Already decided? Subscribe now",
      href: ENTERPRISE_URL,
    },
  },
];

const { universal, differentiators } = collapseUniversalRows(comparison);

function Cell({ value }: { value: ComparisonValue }) {
  if (typeof value === "object") {
    return (
      <a
        href={value.href}
        target="_blank"
        rel="noreferrer"
        className="font-mono text-sm text-brand underline-offset-4 hover:underline"
      >
        {value.text}
      </a>
    );
  }
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
              <p className="mx-auto mt-4 max-w-[640px] text-[15px] leading-relaxed text-muted-foreground">
                Enterprise pricing is a conversation, not a number on a page. Email{" "}
                <a
                  href={PRICING_MAILTO}
                  className="font-mono text-brand underline-offset-4 hover:underline"
                >
                  {PRICING_EMAIL}
                </a>{" "}
                and an engineer will walk you through it.
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
                      target={tier.cta.external ? "_blank" : undefined}
                      rel={tier.cta.external ? "noreferrer" : undefined}
                    >
                      {tier.cta.label}
                    </a>
                  </Button>
                  {tier.secondary ? (
                    <p className="mt-3 text-center text-sm text-muted-foreground">
                      <a
                        href={tier.secondary.href}
                        target="_blank"
                        rel="noreferrer"
                        className="text-brand underline-offset-4 hover:underline"
                      >
                        {tier.secondary.label}
                      </a>
                    </p>
                  ) : null}
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
            <p className="mx-auto mt-5 max-w-[560px] text-[15px] leading-relaxed text-muted-foreground">
              What every tier includes sits on one line at the top, so every row below marks a
              difference between the tiers.
            </p>
          </div>
        </Reveal>

        <Reveal>
          {/* overflow-clip rather than overflow-hidden: hidden would turn this box into the
              sticky header's scroll container and pin it to the box instead of the viewport. */}
          <div className="mx-auto max-w-[980px] overflow-clip rounded-2xl border border-border bg-surface">
            <table className="w-full border-separate border-spacing-0 text-left">
              <thead>
                <tr>
                  <th
                    scope="col"
                    className="sticky top-16 z-10 w-[40%] border-b border-border bg-surface-elevated px-4 sm:px-6 py-4 text-left align-bottom font-mono text-[10px] uppercase tracking-widest text-muted"
                  >
                    Feature
                  </th>
                  {tiers.map((tier) => (
                    <th
                      key={tier.key}
                      scope="col"
                      className="sticky top-16 z-10 border-b border-border bg-surface-elevated px-4 sm:px-6 py-4 text-left align-bottom"
                    >
                      <span className="block font-mono text-[10px] uppercase tracking-widest text-muted">
                        {tier.name}
                      </span>
                      <span className="mt-1.5 flex flex-wrap items-baseline gap-x-1.5 gap-y-0.5">
                        <span className="text-lg font-semibold leading-none tracking-tight text-foreground">
                          {tier.price}
                        </span>
                        <span className="font-mono text-xs text-muted">{tier.cadence}</span>
                      </span>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {universal.length > 0 ? (
                  <tr className="bg-brand-soft/30">
                    <td className="border-b border-border px-4 sm:px-6 py-4 align-top">
                      <span className="block text-[15px] font-medium text-foreground">
                        Core platform, included in every tier
                      </span>
                      <span className="mt-1 block text-sm leading-relaxed text-muted-foreground">
                        {universal.join(" · ")}
                      </span>
                    </td>
                    {tiers.map((tier) => (
                      <td
                        key={tier.key}
                        className="border-b border-border px-4 sm:px-6 py-4 align-top"
                      >
                        <Cell value={true} />
                      </td>
                    ))}
                  </tr>
                ) : null}
                {differentiators.map((group) => (
                  <Fragment key={group.section}>
                    <tr>
                      <td
                        colSpan={tiers.length + 1}
                        className="border-b border-border bg-foreground/[0.015] px-4 sm:px-6 py-3 font-mono text-[10px] uppercase tracking-[0.18em] text-muted-foreground"
                      >
                        {group.section}
                      </td>
                    </tr>
                    {group.rows.map((row) => (
                      <tr key={row.label}>
                        <td className="border-b border-border px-4 sm:px-6 py-4 text-[15px] text-foreground">
                          {row.label}
                        </td>
                        {tiers.map((tier) => (
                          <td key={tier.key} className="border-b border-border px-4 sm:px-6 py-4">
                            <Cell value={row[tier.key]} />
                          </td>
                        ))}
                      </tr>
                    ))}
                  </Fragment>
                ))}
              </tbody>
              <tfoot>
                <tr>
                  <td className="px-4 sm:px-6 py-5 font-mono text-[10px] uppercase tracking-widest text-muted">
                    Next step
                  </td>
                  {tiers.map((tier) => (
                    <td key={tier.key} className="px-4 sm:px-6 py-5 align-top">
                      {/* Wrapping label and full width: a no-wrap button is wider than a
                          phone-width column and the clipped wrapper would cut it off. */}
                      <Button
                        asChild
                        size="sm"
                        variant="outline"
                        className="h-auto w-full whitespace-normal py-2 text-center"
                      >
                        <a
                          href={tier.cta.href}
                          target={tier.cta.external ? "_blank" : undefined}
                          rel={tier.cta.external ? "noreferrer" : undefined}
                        >
                          {tier.cta.label}
                        </a>
                      </Button>
                    </td>
                  ))}
                </tr>
              </tfoot>
            </table>
          </div>
        </Reveal>
      </Section>

      {/* BOTTOM CONTACT */}
      <Section width="narrow" className="border-t border-border/60 py-24 text-center">
        <Reveal>
          <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
            Want to talk numbers?
          </h2>
          <p className="mx-auto mt-5 max-w-[560px] text-base leading-relaxed text-muted-foreground md:text-lg">
            Don't price your build farm off a website. Email{" "}
            <span className="font-mono text-foreground">{PRICING_EMAIL}</span> and the engineers who
            build NativeLink will walk through pricing for your team and workload.
          </p>
          <div className="mt-8 flex flex-wrap justify-center gap-3">
            <Button size="lg" asChild>
              <a href={PRICING_MAILTO}>Email us about pricing</a>
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
