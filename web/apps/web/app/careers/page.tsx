import { Badge, Button, Eyebrow, Reveal, Section } from "@nativelink/ui";

export const metadata = {
  title: "Careers",
  description:
    "Join NativeLink. We are a small team building the open source remote build cache and execution platform, and we are growing.",
};

const roles = [
  {
    name: "Member of Technical Staff",
    location: "London · In-office",
    href: "/careers/members-of-technical-staff",
    summary:
      "Operate large clusters and own real systems end to end. Strong Go and Kubernetes, a love of build systems and low latency, and an interest in Rust. Safety-critical or aerospace experience is a plus, not a requirement.",
  },
];

export default function CareersPage() {
  return (
    <>
      {/* HERO */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[560px] bg-[radial-gradient(ellipse_1000px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.16),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-16 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[900px] text-center">
              <Eyebrow className="mb-5">Careers</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] md:text-[64px]">
                We are making a small team{" "}
                <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
                  bigger
                </span>
                .
              </h1>
              <p className="mx-auto mt-6 max-w-[660px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                We build the open source remote build cache and execution
                platform, and we are growing. If you love build systems, low
                latency, and Rust, we would like to meet you.
              </p>
              <div className="mt-8 flex flex-wrap justify-center gap-3">
                <Button size="lg" asChild>
                  <a href="#open-roles">See open roles</a>
                </Button>
                <Button size="lg" variant="outline" asChild>
                  <a
                    href="https://github.com/TraceMachina/nativelink"
                    target="_blank"
                    rel="noreferrer"
                  >
                    Start with the code
                  </a>
                </Button>
              </div>
            </div>
          </Reveal>
        </Section>
      </section>

      {/* OPEN ROLES */}
      <Section
        id="open-roles"
        width="default"
        className="border-y border-border/60 bg-surface-elevated/40 py-24"
      >
        <Reveal>
          <div className="mb-12 max-w-[680px]">
            <Eyebrow className="mb-4">Open roles</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Come build with us.
            </h2>
            <p className="mt-4 text-[16px] leading-relaxed text-muted-foreground">
              Our current openings are below. We are growing, so more roles, in
              more places, are on the way.
            </p>
          </div>
        </Reveal>
        <div className="grid max-w-[640px] gap-5">
          {roles.map((r, i) => (
            <Reveal key={r.name} delay={i * 0.05}>
              <a
                href={r.href}
                className="group flex h-full flex-col rounded-2xl border border-border bg-surface p-7 transition-colors hover:border-brand/40"
              >
                <div className="flex items-start justify-between gap-4">
                  <h3 className="text-xl font-semibold tracking-tight text-foreground">
                    {r.name}
                  </h3>
                  <Badge variant="brand">Hiring</Badge>
                </div>
                <p className="mt-1 font-mono text-xs uppercase tracking-[0.14em] text-muted">
                  {r.location}
                </p>
                <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                  {r.summary}
                </p>
                <span className="mt-4 text-sm font-medium text-brand">
                  Read the full description →
                </span>
              </a>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* HOW WE HIRE */}
      <Section width="narrow" className="py-24">
        <Reveal>
          <div className="mb-10 max-w-[680px]">
            <Eyebrow className="mb-4">How we hire</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Start with the open source project.
            </h2>
          </div>
        </Reveal>
        <Reveal>
          <div className="space-y-5 text-[16px] leading-relaxed text-muted-foreground">
            <p>
              Our best hires came to the open source project, found a bug, and
              fixed it. That is the strongest signal we know of, and it is the
              best way to reach us. Before you message us, head to the
              repository and find something you like. We will help you get
              started, and improving our contributor onboarding is itself a much
              appreciated contribution.
            </p>
            <p>
              We prioritize a love of build systems, low latency, an interest
              in Rust, and experience with Rust or C++. None of it needs to be
              on a resume: it can be in your commits.
            </p>
          </div>
        </Reveal>
        <Reveal>
          <p className="mt-10 text-sm text-muted">
            Note: recruiters will be blocked and marked as spam.
          </p>
        </Reveal>
      </Section>
    </>
  );
}
