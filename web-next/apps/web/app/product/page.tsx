import {
  Badge,
  Button,
  Eyebrow,
  FAQ,
  type FAQItem,
  Reveal,
  Section,
} from "@nativelink/ui";
import { ArchitectureDiagram } from "@/components/architecture-diagram";

export const metadata = { title: "Product" };

const pillars = [
  {
    eyebrow: "Remote cache",
    title: "Cache once. Reuse forever.",
    body: "Content-addressable storage deduplicates every artifact your team produces. If a teammate, your CI, or an agent has already built it, you get it back in milliseconds. Drops into Bazel, Buck2, Reclient, Pants, Goma — and CMake via recc.",
    metric: "1B+",
    metricLabel: "requests / month",
    icon: (
      <path d="M3 5C3 3 6 3 12 3C18 3 21 3 21 5V19C21 21 18 21 12 21C6 21 3 21 3 19V5Z M3 5C3 7 6 7 12 7C18 7 21 7 21 5 M3 12C3 14 6 14 12 14C18 14 21 14 21 12" strokeLinejoin="round" />
    ),
  },
  {
    eyebrow: "Remote build execution",
    title: "Distribute across every core you have.",
    body: "Offload compilation and tests to a worker fleet that scales horizontally — on AWS, GCP, or bare metal. Hermetic by design, deterministic by default. Specialized hardware (GPUs, ARM, Apple Silicon) supported natively.",
    metric: "10×",
    metricLabel: "average speedup",
    icon: (
      <><circle cx="6" cy="6" r="3" /><circle cx="18" cy="6" r="3" /><circle cx="6" cy="18" r="3" /><circle cx="18" cy="18" r="3" /><path d="M9 6h6M9 18h6M6 9v6M18 9v6" /></>
    ),
  },
  {
    eyebrow: "Cloud & self-host",
    title: "Hosted by us. Or run by you.",
    body: "Start free on NativeLink Cloud in ten minutes. Move to dedicated infrastructure when your scale demands it. Or self-host the open-source release with one Docker command. Same code path. Same performance.",
    metric: "<10m",
    metricLabel: "time to first hit",
    icon: (
      <path d="M6 18H4C2.9 18 2 17.1 2 16C2 14.9 2.9 14 4 14H4.2C4.7 11.7 6.7 10 9 10C11.3 10 13.3 11.7 13.8 14H14C15.1 14 16 14.9 16 16C16 17.1 15.1 18 14 18H6Z M16 13L18 11L20 13M18 11V18" strokeLinejoin="round" strokeLinecap="round" />
    ),
  },
  {
    eyebrow: "Built on Rust",
    title: "Performance that doesn't stall.",
    body: "No garbage collector. No race conditions at scale. No mystery latency spikes. Memory safety without the runtime tax — which is why NativeLink can serve a billion requests a month on infrastructure that would buckle other systems.",
    metric: "0",
    metricLabel: "GC pauses",
    icon: (
      <path d="M12 3L4 7v6c0 4 4 7 8 8 4-1 8-4 8-8V7L12 3Z M9 12l2 2 4-4" strokeLinejoin="round" strokeLinecap="round" />
    ),
  },
];

const integrationGroups = [
  {
    label: "AI coding platforms",
    items: ["Claude Code", "Copilot Workspace", "Devin", "Cursor", "Windsurf"],
  },
  { label: "Languages", items: ["C++", "Rust", "Python", "Go", "Java", "Kotlin", "Swift"] },
  {
    label: "Build systems",
    items: ["Bazel", "Buck2", "Reclient", "Soong", "Pants", "Goma", "CMake (recc)"],
  },
  { label: "Cloud", items: ["AWS", "GCP", "Azure", "Bare metal"] },
  { label: "CI", items: ["GitHub Actions", "GitLab", "Buildkite", "Jenkins"] },
  { label: "Storage", items: ["S3", "GCS", "Redis", "Local", "Memory"] },
];

const securityPoints = [
  { label: "SSO / SAML", body: "Okta, Azure AD, Google Workspace." },
  { label: "End-to-end TLS", body: "mTLS between every hop in the pipeline." },
  { label: "Content-addressed", body: "Every artifact tamper-evident by hash." },
  { label: "Signed inputs", body: "Worker inputs and outputs cryptographically signed." },
  { label: "Hermetic builds", body: "No surprise dependencies pulled mid-build." },
  { label: "Audit trails", body: "Queryable provenance for every action result." },
];

const faqItems: FAQItem[] = [
  {
    q: "What is NativeLink?",
    a: "A high-performance build cache and remote execution system designed to accelerate compilation and testing while cutting infrastructure spend.",
  },
  {
    q: "How do I set up NativeLink?",
    a: "Deploy it as a Docker image — the documentation walks through every step, from a single-node local setup to a multi-region cluster.",
  },
  {
    q: "What operating systems are supported?",
    a: "Linux, macOS, and Windows for clients. Linux and macOS for the server. Workers can target any platform your toolchains support.",
  },
  {
    q: "Is NativeLink free?",
    a: "The open-source core is MIT-licensed and free forever. You only pay if you want our managed Cloud or an Enterprise contract.",
  },
  {
    q: "Why Rust?",
    a: "Memory safety without garbage collection. No mystery pauses, no race conditions at scale, no runtime tax — which is how a single NativeLink cluster serves over a billion requests a month.",
  },
  {
    q: "How does remote execution work?",
    a: "NativeLink distributes build and test actions across a network of workers. The scheduler decides who runs what, hermetically, and content-addresses every input and output.",
  },
  {
    q: "Does it work with my existing tools?",
    a: "Yes. Anything that speaks the Remote Execution API — Bazel, Buck2, Reclient, Goma, Pants, or CMake via recc — works without modification.",
  },
  {
    q: "How do I keep my Bazel setup hermetic?",
    a: "NativeLink eliminates external dependencies and enforces consistency across builds. The docs cover the configuration in depth.",
  },
  {
    q: "Why should I choose NativeLink?",
    a: "Trusted by some of the largest engineering organizations in the world for cutting both build wall-time and CI compute cost. Handles over one billion requests a month, designed for scale.",
  },
];

export default function ProductPage() {
  return (
    <>
      {/* HERO */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[600px] bg-[radial-gradient(ellipse_1000px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.18),transparent_70%)]" />

        <Section width="default" className="pt-24 pb-12 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[900px] text-center">
              <Eyebrow className="mb-5">Product</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] md:text-[68px]">
                One platform.{" "}
                <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
                  Every build. Every machine.
                </span>
              </h1>
              <p className="mx-auto mt-6 max-w-[700px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                NativeLink unifies remote caching, remote execution, and observability
                into a single Rust-native platform — built to keep up with codebases
                that grow faster than you can provision them.
              </p>
              <div className="mt-8 flex flex-wrap justify-center gap-3">
                <Button size="lg" asChild>
                  <a href="/docs">Start in 10 minutes</a>
                </Button>
                <Button size="lg" variant="outline" asChild>
                  <a href="mailto:contact@nativelink.com">Talk to us</a>
                </Button>
              </div>
            </div>
          </Reveal>
        </Section>

        {/* Architecture diagram */}
        <Section width="default" className="pb-20">
          <Reveal>
            <ArchitectureDiagram />
          </Reveal>
        </Section>
      </section>

      {/* PILLARS */}
      <Section width="default" className="border-t border-border/60 bg-surface-elevated/40 py-24">
        <Reveal>
          <div className="mb-14 max-w-[600px]">
            <Eyebrow className="mb-4">The platform</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Four pillars. One engine.
            </h2>
          </div>
        </Reveal>

        <div className="grid gap-4 md:grid-cols-2">
          {pillars.map((p, i) => (
            <Reveal key={p.title} delay={i * 0.06}>
              <article className="group relative h-full overflow-hidden rounded-2xl border border-border bg-surface p-8 transition-all hover:border-brand/40 hover:shadow-[0_24px_60px_-30px_rgb(var(--nl-color-brand)/0.4)]">
                <div className="flex items-start justify-between">
                  <div className="flex h-11 w-11 items-center justify-center rounded-xl bg-brand-soft text-brand transition-colors group-hover:bg-brand group-hover:text-brand-foreground">
                    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
                      {p.icon}
                    </svg>
                  </div>
                  <div className="text-right">
                    <div className="font-mono text-2xl font-semibold tracking-tight text-foreground">
                      {p.metric}
                    </div>
                    <div className="font-mono text-[10px] uppercase tracking-widest text-muted">
                      {p.metricLabel}
                    </div>
                  </div>
                </div>
                <Eyebrow tone="muted" className="mt-6 text-[10px]">
                  {p.eyebrow}
                </Eyebrow>
                <h3 className="mt-2 text-2xl font-semibold leading-tight tracking-tight text-foreground">
                  {p.title}
                </h3>
                <p className="mt-4 text-[15px] leading-relaxed text-muted-foreground">
                  {p.body}
                </p>
              </article>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* SECURITY */}
      <Section width="default" className="border-t border-border/60 py-28">
        <div className="grid gap-12 lg:grid-cols-[1fr_1.4fr] lg:gap-16">
          <Reveal>
            <Eyebrow className="mb-4">Security & provenance</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Trust at every hop.{" "}
              <span className="text-brand">Provenance at every step.</span>
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted-foreground md:text-lg">
              When humans commit code, you need traceability. When agents commit code,
              you need it doubly. NativeLink makes every input explicit and every
              output verifiable.
            </p>
            <div className="mt-8 flex flex-wrap gap-2">
              <Badge variant="success">SOC 2 in progress</Badge>
              <Badge variant="outline">mTLS</Badge>
              <Badge variant="outline">SSO/SAML</Badge>
              <Badge variant="outline">Audit logs</Badge>
            </div>
          </Reveal>

          <Reveal delay={0.1}>
            <ul className="grid gap-px overflow-hidden rounded-2xl border border-border bg-border sm:grid-cols-2">
              {securityPoints.map((s) => (
                <li
                  key={s.label}
                  className="flex flex-col gap-2 bg-surface px-6 py-5"
                >
                  <span className="flex items-center gap-2 font-mono text-sm font-semibold text-foreground">
                    <span className="inline-flex h-4 w-4 items-center justify-center rounded-full bg-brand-soft text-[10px] text-brand">
                      ✓
                    </span>
                    {s.label}
                  </span>
                  <span className="text-sm leading-relaxed text-muted-foreground">
                    {s.body}
                  </span>
                </li>
              ))}
            </ul>
          </Reveal>
        </div>
      </Section>

      {/* INTEGRATIONS */}
      <Section width="default" className="border-t border-border/60 bg-surface-elevated/40 py-24">
        <Reveal>
          <div className="mx-auto mb-12 max-w-[680px] text-center">
            <Eyebrow className="mb-4">Integrations</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Works with everything your team already uses.
            </h2>
          </div>
        </Reveal>
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {integrationGroups.map((g, i) => (
            <Reveal key={g.label} delay={i * 0.04}>
              <div className="h-full rounded-2xl border border-border bg-surface p-6">
                <Eyebrow tone="muted" className="mb-4 text-[10px]">
                  {g.label}
                </Eyebrow>
                <ul className="flex flex-wrap gap-1.5">
                  {g.items.map((item) => (
                    <li
                      key={item}
                      className="rounded-md border border-border bg-surface-elevated px-2.5 py-1 font-mono text-xs text-foreground transition-colors hover:border-brand/40 hover:text-brand"
                    >
                      {item}
                    </li>
                  ))}
                </ul>
              </div>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* LLVM CASE STUDY */}
      <section className="relative overflow-hidden border-t border-border/60">
        <div className="pointer-events-none absolute inset-0 -z-10 bg-[radial-gradient(ellipse_at_center,rgb(var(--nl-color-brand)/0.1),transparent_60%)]" />
        <Section width="narrow" className="py-28 text-center">
          <Reveal>
            <Eyebrow className="mb-5">Proof at scale</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-[56px]">
              LLVM builds <span className="text-brand">4× faster</span> on NativeLink.
            </h2>
            <p className="mx-auto mt-6 max-w-[620px] text-base leading-relaxed text-muted-foreground md:text-lg">
              LLVM contributors are using NativeLink with CMake and recc to distribute
              builds of clang and the LLVM toolchain — cutting full-project compile
              time from 17 minutes to 4. No build-system migration. No proprietary
              client. Just your existing CMake setup, pointed at NativeLink.
            </p>
            <div className="mt-10 grid grid-cols-3 gap-6">
              <div>
                <div className="font-mono text-3xl font-semibold tracking-tight text-brand md:text-4xl">
                  4×
                </div>
                <div className="mt-1 text-xs text-muted">faster builds</div>
              </div>
              <div className="border-x border-border/60">
                <div className="font-mono text-3xl font-semibold tracking-tight text-foreground md:text-4xl">
                  17m → 4m
                </div>
                <div className="mt-1 text-xs text-muted">full project compile</div>
              </div>
              <div>
                <div className="font-mono text-3xl font-semibold tracking-tight text-foreground md:text-4xl">
                  0
                </div>
                <div className="mt-1 text-xs text-muted">build-system rewrite</div>
              </div>
            </div>
            <div className="mt-10 flex justify-center">
              <Button variant="link" asChild>
                <a
                  href="https://reidkleckner.dev/posts/llvm-recc-nativelink/"
                  target="_blank"
                  rel="noreferrer"
                >
                  Read the full write-up <span aria-hidden="true">→</span>
                </a>
              </Button>
            </div>
          </Reveal>
        </Section>
      </section>

      {/* FAQ */}
      <Section width="default" className="border-t border-border/60 py-28">
        <Reveal>
          <div className="mx-auto mb-12 max-w-[680px] text-center">
            <Eyebrow className="mb-4">FAQ</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Frequently asked questions
            </h2>
          </div>
        </Reveal>
        <Reveal>
          <div className="mx-auto max-w-[840px]">
            <FAQ items={faqItems} />
            <p className="mt-8 text-center text-sm text-muted-foreground">
              For more details, see the{" "}
              <a href="/docs" className="text-brand underline-offset-4 hover:underline">
                NativeLink documentation
              </a>
              .
            </p>
          </div>
        </Reveal>
      </Section>
    </>
  );
}
