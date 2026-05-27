import {
  Badge,
  Button,
  Counter,
  Eyebrow,
  HeroVisual,
  Marquee,
  Reveal,
  Section,
} from "@nativelink/ui";

export const metadata = {
  title: "NativeLink — Remote build execution & caching, engineered for scale",
};

const features = [
  {
    n: "01",
    title: "Faster builds. Lower compute bills.",
    body: "Cache and parallelize Bazel, Buck2, Reclient, and Pants across your fleet. Cut build wall-time without provisioning a fleet of CI runners.",
  },
  {
    n: "02",
    title: "Scale past one machine.",
    body: "Distribute work across hundreds of cores on commodity infra. Pay only for what you use — the scheduler decides.",
  },
  {
    n: "03",
    title: "Memory-safe by design.",
    body: "Written in Rust. No garbage collector to stall your hot path. Over a billion build requests a month in production.",
  },
  {
    n: "04",
    title: "Open source, batteries included.",
    body: "MIT-licensed. Self-host on your infra in minutes — or let us run it for you. Same engine either way.",
  },
];

const integrations = [
  "Bazel",
  "Buck2",
  "Reclient",
  "Pants",
  "Goma",
  "CMake",
  "Cargo",
  "Soong",
];

const industries = [
  "Robotics & autonomous systems",
  "Semiconductors & EDA",
  "Consumer electronics & mobile",
  "AI & ML research",
  "Financial services",
  "Life sciences",
];

const comparisonRows = [
  { label: "Cold-build time", before: "17 min", after: "4 min" },
  { label: "Cache hit on warm rebuild", before: "0%", after: "94%" },
  { label: "CI compute spend", before: "$$$ baseline", after: "−72%" },
  { label: "Time to first contributor build", before: "45 min", after: "8 min" },
];

const useCases = [
  {
    tag: "Robotics",
    title: "Simulating fleets at machine speed.",
    body: "Autonomous-vehicle teams replay terabytes of sensor data against every commit. NativeLink keeps the wheel-to-wheel feedback under a minute.",
  },
  {
    tag: "Silicon",
    title: "Verification that keeps up with tape-out.",
    body: "EDA flows fan out to thousands of cores. NativeLink hands every job to the next free machine — and caches the result for the next sprint.",
  },
  {
    tag: "Agents",
    title: "Build infrastructure for the agentic era.",
    body: "When an LLM commits a thousand PRs a day, your build farm is the bottleneck. NativeLink turns it into the part that never blinks.",
  },
];

export default function HomePage() {
  return (
    <>
      {/* ── HERO ─────────────────────────────────────────────────────────── */}
      <section className="relative overflow-hidden">
        {/* Layered backdrops */}
        <div className="pointer-events-none absolute inset-0 -z-10 bg-brand-glow" />
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[800px] bg-[radial-gradient(ellipse_1100px_600px_at_80%_-5%,rgb(var(--nl-color-brand)/0.18),transparent_70%)]" />
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[600px] bg-dot-grid opacity-50 [mask-image:radial-gradient(ellipse_at_top,black_20%,transparent_70%)]" />

        <Section width="default" className="pt-20 pb-24 md:pt-28">
          <div className="grid items-center gap-14 lg:grid-cols-[1fr_1.1fr]">
            <Reveal>
              <div className="mb-6 inline-flex items-center gap-2 rounded-full border border-border bg-surface/70 px-3 py-1 text-xs backdrop-blur">
                <span className="relative flex h-1.5 w-1.5">
                  <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-brand opacity-60" />
                  <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-brand" />
                </span>
                <span className="font-mono uppercase tracking-[0.14em] text-muted-foreground">
                  v1.3 · Now with persistent workers
                </span>
              </div>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] text-foreground md:text-[68px]">
                Builds that finish{" "}
                <span className="bg-gradient-to-r from-brand via-brand to-brand-strong bg-clip-text text-transparent">
                  before you can blink.
                </span>
              </h1>
              <p className="mt-6 max-w-[560px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                NativeLink caches and parallelizes Bazel, Buck2, Reclient, and Pants at
                infrastructure scale. Written in Rust. Open source. Battle-tested on
                over a billion build requests a month.
              </p>
              <div className="mt-8 flex flex-wrap items-center gap-3">
                <Button size="lg" asChild>
                  <a href="/docs">
                    Get started <span aria-hidden="true">→</span>
                  </a>
                </Button>
                <Button size="lg" variant="outline" asChild>
                  <a href="/product">See the platform</a>
                </Button>
              </div>
              <div className="mt-6 flex flex-wrap items-center gap-x-5 gap-y-2 text-xs text-muted">
                <span className="flex items-center gap-1.5">
                  <span className="inline-block h-1.5 w-1.5 rounded-full bg-success" />
                  Free forever tier
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="inline-block h-1.5 w-1.5 rounded-full bg-success" />
                  Self-host or cloud
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="inline-block h-1.5 w-1.5 rounded-full bg-success" />
                  No credit card
                </span>
              </div>
            </Reveal>

            <Reveal delay={0.15}>
              <HeroVisual />
            </Reveal>
          </div>
        </Section>

        {/* Marquee strip */}
        <div className="relative border-y border-border/60 bg-surface-elevated/40 py-6">
          <p className="mb-3 text-center font-mono text-[11px] uppercase tracking-[0.18em] text-muted">
            Speaks every major build system
          </p>
          <Marquee speed={45}>
            {integrations.map((name) => (
              <span
                key={name}
                className="text-xl font-semibold text-muted-foreground transition-colors hover:text-brand md:text-2xl"
              >
                {name}
              </span>
            ))}
          </Marquee>
        </div>
      </section>

      {/* ── STATS ────────────────────────────────────────────────────────── */}
      <Section width="default" className="border-b border-border/60 py-20">
        <Reveal>
          <div className="grid grid-cols-2 gap-x-8 gap-y-10 md:grid-cols-4">
            <div className="flex flex-col gap-2">
              <div className="font-mono text-4xl font-semibold leading-none tracking-tight text-brand md:text-5xl">
                <Counter to={1} suffix="B+" />
              </div>
              <div className="text-sm leading-relaxed text-muted">
                Build requests served per month
              </div>
            </div>
            <div className="flex flex-col gap-2">
              <div className="font-mono text-4xl font-semibold leading-none tracking-tight text-foreground md:text-5xl">
                <Counter to={10} suffix="×" />
              </div>
              <div className="text-sm leading-relaxed text-muted">
                Cache hit speedup vs cold build
              </div>
            </div>
            <div className="flex flex-col gap-2">
              <div className="font-mono text-4xl font-semibold leading-none tracking-tight text-foreground md:text-5xl">
                &lt;<Counter to={10} suffix="m" />
              </div>
              <div className="text-sm leading-relaxed text-muted">
                From clone to first cache hit
              </div>
            </div>
            <div className="flex flex-col gap-2">
              <div className="font-mono text-4xl font-semibold leading-none tracking-tight text-brand md:text-5xl">
                <Counter to={100} suffix="%" />
              </div>
              <div className="text-sm leading-relaxed text-muted">
                Open source core (MIT)
              </div>
            </div>
          </div>
        </Reveal>
      </Section>

      {/* ── WHY (numbered card grid) ─────────────────────────────────────── */}
      <Section width="default" className="py-28">
        <Reveal>
          <div className="mx-auto mb-16 max-w-[720px] text-center">
            <Eyebrow className="mb-4">Why NativeLink</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-[56px]">
              Build infrastructure that{" "}
              <span className="text-brand">disappears</span>.
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted-foreground md:text-lg">
              The fastest builds are the ones you don't notice. NativeLink runs quietly
              behind your existing build system, doing the work so your engineers don't
              have to wait.
            </p>
          </div>
        </Reveal>

        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          {features.map((f, i) => (
            <Reveal key={f.title} delay={i * 0.06}>
              <div className="group relative h-full overflow-hidden rounded-2xl border border-border bg-surface p-7 transition-all hover:-translate-y-0.5 hover:border-brand/40 hover:shadow-[0_20px_50px_-25px_rgb(var(--nl-color-brand)/0.4)]">
                <div className="absolute right-5 top-5 font-mono text-[11px] tracking-[0.15em] text-muted/60">
                  {f.n}
                </div>
                <div className="mb-5 inline-flex h-10 w-10 items-center justify-center rounded-xl bg-brand-soft text-brand transition-colors group-hover:bg-brand group-hover:text-brand-foreground">
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" aria-hidden="true">
                    {i === 0 && <path d="M13 2 L4 14 H12 L11 22 L20 10 H12 Z" strokeLinejoin="round" />}
                    {i === 1 && <><circle cx="6" cy="6" r="2.5" /><circle cx="18" cy="6" r="2.5" /><circle cx="6" cy="18" r="2.5" /><circle cx="18" cy="18" r="2.5" /><path d="M8 6h8M8 18h8M6 8v8M18 8v8" /></>}
                    {i === 2 && <path d="M12 3 L4 7 V13 C4 17 8 20 12 21 C16 20 20 17 20 13 V7 Z" strokeLinejoin="round" />}
                    {i === 3 && <path d="M16 18 L22 12 L16 6 M8 6 L2 12 L8 18 M14 4 L10 20" strokeLinecap="round" strokeLinejoin="round" />}
                  </svg>
                </div>
                <h3 className="text-lg font-semibold leading-tight tracking-tight text-foreground">
                  {f.title}
                </h3>
                <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                  {f.body}
                </p>
              </div>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* ── BEFORE / AFTER COMPARISON ────────────────────────────────────── */}
      <Section width="default" className="border-y border-border/60 bg-surface-elevated/40 py-24">
        <Reveal>
          <div className="mx-auto mb-12 max-w-[720px] text-center">
            <Eyebrow className="mb-4">Proof at scale</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              From hours to minutes. From bills to bytes.
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted-foreground md:text-lg">
              What an LLVM contributor sees on day one with NativeLink.
            </p>
          </div>
        </Reveal>

        <Reveal>
          <div className="mx-auto max-w-[960px] overflow-hidden rounded-2xl border border-border bg-surface">
            <div className="grid grid-cols-[1fr_auto_1fr] items-center border-b border-border bg-surface-elevated px-6 py-4">
              <div className="text-left">
                <p className="font-mono text-[10px] uppercase tracking-widest text-muted">
                  Before
                </p>
                <p className="font-mono text-sm text-muted-foreground">
                  CMake + local cores
                </p>
              </div>
              <div className="font-mono text-xs text-muted">→</div>
              <div className="text-right">
                <p className="font-mono text-[10px] uppercase tracking-widest text-brand">
                  With NativeLink
                </p>
                <p className="font-mono text-sm text-foreground">
                  CMake + recc + cache
                </p>
              </div>
            </div>
            {comparisonRows.map((row, i) => (
              <div
                key={row.label}
                className="grid grid-cols-[1fr_auto_1fr] items-center gap-4 border-t border-border first:border-t-0 px-6 py-5 first:bg-transparent"
                style={i % 2 === 1 ? { background: "rgb(var(--nl-color-foreground) / 0.02)" } : undefined}
              >
                <div className="font-mono text-lg text-muted-foreground line-through decoration-foreground/30">
                  {row.before}
                </div>
                <div className="text-center text-xs text-muted">
                  {row.label}
                </div>
                <div className="text-right font-mono text-lg font-semibold text-brand">
                  {row.after}
                </div>
              </div>
            ))}
          </div>
        </Reveal>
      </Section>

      {/* ── USE CASES (asymmetric grid) ──────────────────────────────────── */}
      <Section width="default" className="py-28">
        <Reveal>
          <div className="mb-14 flex flex-col gap-6 md:flex-row md:items-end md:justify-between">
            <div className="max-w-[600px]">
              <Eyebrow className="mb-4">Who runs NativeLink</Eyebrow>
              <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
                The teams shipping at{" "}
                <span className="text-brand">machine speed</span>.
              </h2>
            </div>
            <p className="max-w-[420px] text-base leading-relaxed text-muted-foreground">
              From silicon verification to autonomous-vehicle simulation, NativeLink
              powers the build farms that can't blink.
            </p>
          </div>
        </Reveal>

        <div className="grid gap-5 lg:grid-cols-3">
          {useCases.map((uc, i) => (
            <Reveal key={uc.title} delay={i * 0.06}>
              <article className="group flex h-full flex-col rounded-2xl border border-border bg-surface p-8 transition-colors hover:border-border-strong">
                <Badge variant="brand" className="self-start">
                  {uc.tag}
                </Badge>
                <h3 className="mt-6 text-2xl font-semibold leading-tight tracking-tight text-foreground">
                  {uc.title}
                </h3>
                <p className="mt-4 flex-1 text-[15px] leading-relaxed text-muted-foreground">
                  {uc.body}
                </p>
                <div className="mt-6 inline-flex items-center gap-1.5 font-mono text-sm text-brand opacity-0 transition-opacity group-hover:opacity-100">
                  Read case study <span aria-hidden="true">→</span>
                </div>
              </article>
            </Reveal>
          ))}
        </div>

        <Reveal>
          <div className="mt-8 grid grid-cols-2 gap-x-8 gap-y-3 rounded-2xl border border-border bg-surface-elevated/60 p-6 sm:grid-cols-3 md:grid-cols-6">
            {industries.map((i) => (
              <p key={i} className="font-mono text-xs text-muted-foreground">
                {i}
              </p>
            ))}
          </div>
        </Reveal>
      </Section>

      {/* ── QUOTE BAND ───────────────────────────────────────────────────── */}
      <section className="relative overflow-hidden border-y border-border/60">
        <div className="pointer-events-none absolute inset-0 -z-10 bg-[radial-gradient(ellipse_at_center,rgb(var(--nl-color-brand)/0.12),transparent_60%)]" />
        <Section width="narrow" className="py-28 text-center">
          <Reveal>
            <Eyebrow className="mb-6">Demonstrated on LLVM</Eyebrow>
            <blockquote className="text-balance text-3xl font-semibold leading-[1.2] tracking-[-0.02em] md:text-[44px]">
              "Engineered for the world's largest C++ codebases —{" "}
              <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
                and yours.
              </span>
              "
            </blockquote>
            <p className="mt-6 text-sm text-muted-foreground">
              Production-tested on LLVM, one of the largest open-source codebases in the world.
            </p>
            <div className="mt-8 flex flex-wrap justify-center gap-2">
              <Badge variant="brand">
                <Counter to={4} />× faster builds
              </Badge>
              <Badge variant="outline">17 min → 4 min</Badge>
              <Badge variant="outline">CMake + recc</Badge>
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
    </>
  );
}
