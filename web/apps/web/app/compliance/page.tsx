import { Badge, Button, Eyebrow, Reveal, Section } from "@nativelink/ui";

export const metadata = {
  title: "Compliance",
  description:
    "NativeLink's security posture — SOC 2, GDPR, encryption, audit logs, signed artifacts, and more.",
};

const certifications = [
  {
    name: "SOC 2 Type II",
    status: "In progress",
    statusTone: "brand" as const,
    body: "Audit underway, expected report Q3 2026. We're tracking against the AICPA Trust Services Criteria today.",
  },
  {
    name: "GDPR",
    status: "Compliant",
    statusTone: "success" as const,
    body: "Data Processing Addendum available on request. EU data residency for Cloud customers.",
  },
  {
    name: "CCPA",
    status: "Compliant",
    statusTone: "success" as const,
    body: "California residents can request access, deletion, and opt-out at any time.",
  },
  {
    name: "ISO 27001",
    status: "Planned",
    statusTone: "default" as const,
    body: "Targeting 2027 once our SOC 2 Type II report is published.",
  },
];

const controls = [
  {
    icon: (
      <path d="M12 2 L4 6V13 C4 17 8 20 12 22 C16 20 20 17 20 13 V6 L12 2 Z M9 12 L11 14 L15 10" strokeLinecap="round" strokeLinejoin="round" />
    ),
    title: "Encryption everywhere",
    body: "TLS 1.3 in transit. AES-256 at rest. mTLS between every service in the cluster. Customer keys supported on Enterprise.",
  },
  {
    icon: (
      <><circle cx="12" cy="8" r="3" /><path d="M5 21 v-2 c0-3 3-5 7-5 s7 2 7 5 v2" /></>
    ),
    title: "Single sign-on",
    body: "SAML 2.0 and OIDC with Okta, Azure AD, Google Workspace, JumpCloud. SCIM 2.0 user provisioning on Enterprise.",
  },
  {
    icon: (
      <><rect x="4" y="6" width="16" height="14" rx="2" /><path d="M8 6V4 a2 2 0 0 1 2-2 h4 a2 2 0 0 1 2 2 v2 M10 13 h4 M10 17 h4" strokeLinecap="round" /></>
    ),
    title: "Audit logs",
    body: "Every administrative action and every action result is logged, signed, and exportable to your SIEM via webhook or S3.",
  },
  {
    icon: (
      <path d="M12 22 c-4 0-7-3-7-7 V7 L12 4 L19 7 v8 c0 4-3 7-7 7 Z M8 12 h8 M12 8 v8" strokeLinecap="round" strokeLinejoin="round" />
    ),
    title: "Signed artifacts",
    body: "Build inputs and outputs are content-addressed and cryptographically signed. Tampering is detectable at the hash level.",
  },
  {
    icon: (
      <><path d="M12 2 L2 12 L12 22 L22 12 Z" /><circle cx="12" cy="12" r="3" /></>
    ),
    title: "Data residency",
    body: "US and EU regions on Cloud. Pin your data to a region by contract. Enterprise customers can run fully air-gapped.",
  },
  {
    icon: (
      <path d="M3 12 L9 6 L21 18 M15 6 L21 6 L21 12" strokeLinecap="round" strokeLinejoin="round" />
    ),
    title: "Vulnerability program",
    body: "We run continuous SAST/DAST on every PR. Disclose anything at security@nativelink.com — we triage within 24h.",
  },
];

const policies = [
  { name: "Information Security Policy", description: "Our internal standards for handling customer and corporate data." },
  { name: "Incident Response Plan", description: "How we detect, escalate, and communicate during an incident." },
  { name: "Sub-processor list", description: "The vendors that process customer data on our behalf." },
  { name: "Data Processing Addendum", description: "GDPR-compliant DPA for European customers." },
];

export default function CompliancePage() {
  return (
    <>
      {/* HERO */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[500px] bg-[radial-gradient(ellipse_900px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.13),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-16 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[860px] text-center">
              <Eyebrow className="mb-5">Compliance</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] md:text-[64px]">
                Security you can{" "}
                <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
                  audit, not just trust
                </span>
                .
              </h1>
              <p className="mx-auto mt-6 max-w-[640px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                NativeLink runs the build farms that ship safety-critical software.
                Every byte is content-addressed, every action is logged, and every
                control is auditable.
              </p>
              <div className="mt-8 flex flex-wrap justify-center gap-3">
                <Button size="lg" asChild>
                  <a href="mailto:security@nativelink.com">Request a report</a>
                </Button>
                <Button size="lg" variant="outline" asChild>
                  <a href="/contact">Contact security</a>
                </Button>
              </div>
            </div>
          </Reveal>
        </Section>
      </section>

      {/* CERTIFICATIONS */}
      <Section width="default" className="border-y border-border/60 bg-surface-elevated/40 py-24">
        <Reveal>
          <div className="mb-12 max-w-[680px]">
            <Eyebrow className="mb-4">Certifications</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Where we are. Where we're going.
            </h2>
          </div>
        </Reveal>
        <div className="grid gap-5 md:grid-cols-2">
          {certifications.map((c, i) => (
            <Reveal key={c.name} delay={i * 0.05}>
              <div className="flex h-full flex-col rounded-2xl border border-border bg-surface p-7">
                <div className="flex items-start justify-between gap-4">
                  <h3 className="text-xl font-semibold tracking-tight text-foreground">
                    {c.name}
                  </h3>
                  <Badge variant={c.statusTone}>{c.status}</Badge>
                </div>
                <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                  {c.body}
                </p>
              </div>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* CONTROLS */}
      <Section width="default" className="py-28">
        <Reveal>
          <div className="mx-auto mb-14 max-w-[680px] text-center">
            <Eyebrow className="mb-4">Controls</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              How NativeLink protects your code.
            </h2>
          </div>
        </Reveal>
        <div className="grid gap-5 sm:grid-cols-2 lg:grid-cols-3">
          {controls.map((c, i) => (
            <Reveal key={c.title} delay={i * 0.04}>
              <div className="group h-full rounded-2xl border border-border bg-surface p-6 transition-all hover:border-brand/40">
                <div className="mb-4 inline-flex h-10 w-10 items-center justify-center rounded-xl bg-brand-soft text-brand transition-colors group-hover:bg-brand group-hover:text-brand-foreground">
                  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
                    {c.icon}
                  </svg>
                </div>
                <h3 className="text-lg font-semibold leading-tight tracking-tight text-foreground">
                  {c.title}
                </h3>
                <p className="mt-2 text-[15px] leading-relaxed text-muted-foreground">
                  {c.body}
                  {c.title === "Data residency" && (
                    <>
                      {" "}
                      <a
                        href="https://enterprise.nativelink.com"
                        target="_blank"
                        rel="noreferrer"
                        className="font-medium text-brand underline-offset-4 hover:underline"
                      >
                        Deploy on-prem with our Helm charts →
                      </a>
                    </>
                  )}
                </p>
              </div>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* POLICIES */}
      <Section width="default" className="border-t border-border/60 py-24">
        <Reveal>
          <div className="mb-10 flex flex-col gap-6 md:flex-row md:items-end md:justify-between">
            <div className="max-w-[600px]">
              <Eyebrow className="mb-4">Policies & reports</Eyebrow>
              <h2 className="text-balance text-3xl font-semibold leading-[1.1] tracking-[-0.025em] md:text-4xl">
                Available on request.
              </h2>
              <p className="mt-5 text-base leading-relaxed text-muted-foreground md:text-lg">
                Email{" "}
                <a href="mailto:security@nativelink.com" className="text-brand underline-offset-4 hover:underline">
                  security@nativelink.com
                </a>{" "}
                with your NDA — we'll send the relevant report within one business
                day.
              </p>
            </div>
          </div>
        </Reveal>

        <Reveal>
          <ul className="divide-y divide-border overflow-hidden rounded-2xl border border-border bg-surface">
            {policies.map((p) => (
              <li key={p.name} className="flex items-center justify-between gap-6 px-6 py-5 transition-colors hover:bg-foreground/[0.02]">
                <div>
                  <p className="font-mono text-sm font-semibold text-foreground">
                    {p.name}
                  </p>
                  <p className="mt-1 text-sm text-muted-foreground">{p.description}</p>
                </div>
                <a
                  href="mailto:security@nativelink.com"
                  className="inline-flex items-center gap-1.5 font-mono text-sm text-brand transition-all hover:gap-2"
                >
                  Request <span aria-hidden="true">→</span>
                </a>
              </li>
            ))}
          </ul>
        </Reveal>
      </Section>

      {/* RESPONSIBLE DISCLOSURE */}
      <section className="relative overflow-hidden border-t border-border/60">
        <div className="pointer-events-none absolute inset-0 -z-10 bg-[radial-gradient(ellipse_at_center,rgb(var(--nl-color-brand)/0.1),transparent_60%)]" />
        <Section width="narrow" className="py-24 text-center">
          <Reveal>
            <Eyebrow className="mb-5">Responsible disclosure</Eyebrow>
            <h2 className="text-balance text-3xl font-semibold leading-[1.1] tracking-[-0.025em] md:text-4xl">
              Found something? We want to hear about it.
            </h2>
            <p className="mx-auto mt-5 max-w-[560px] text-base leading-relaxed text-muted-foreground md:text-lg">
              We triage every report within 24 hours, fix high-severity issues
              within 7 days, and credit researchers in our advisory.
            </p>
            <div className="mt-8 flex justify-center">
              <Button size="lg" asChild>
                <a href="mailto:security@nativelink.com">security@nativelink.com</a>
              </Button>
            </div>
          </Reveal>
        </Section>
      </section>
    </>
  );
}
