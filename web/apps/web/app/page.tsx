import { ArchitectureDiagram } from "@/components/architecture-diagram";
import {
  CitrixLogo,
  MenloSecurityLogo,
  MetaLogo,
  SamsungLogo,
  TeslaLogo,
  ThirdWaveLogo,
} from "@/components/customer-logos";
import { McpDemo } from "@/components/mcp-demo";
import { Badge, Button, Eyebrow, HeroVisual, Marquee, Reveal, Section } from "@nativelink/ui";

export const metadata = {
  title: "NativeLink — Build infrastructure for the agentic era",
};

const integrations = [
  "Bazel",
  "Buck2",
  "Reclient",
  "Buildstream",
  "recc",
  "GN",
  "Siso",
  "Pants",
  "Goma",
  "CMake",
  "Cargo",
  "Soong",
];

const customerLogos = [
  {
    name: "Menlo Security",
    href: "https://www.menlosecurity.com/",
    Logo: MenloSecurityLogo,
    className: "h-9 md:h-10",
  },
  {
    name: "Citrix",
    href: "https://www.citrix.com/",
    Logo: CitrixLogo,
    className: "h-7 md:h-8",
  },
  {
    name: "Tesla",
    href: "https://www.tesla.com/",
    Logo: TeslaLogo,
    className: "h-6 md:h-7",
  },
  {
    name: "Meta",
    href: "https://www.meta.com/about/",
    Logo: MetaLogo,
    className: "h-7 md:h-8",
  },
  {
    name: "Samsung",
    href: "https://www.samsung.com/",
    Logo: SamsungLogo,
    className: "h-6 md:h-7",
  },
  {
    name: "Third Wave",
    href: "https://thirdwave.ai/",
    Logo: ThirdWaveLogo,
    className: "h-10 md:h-11",
  },
];

const stats = [
  {
    value: "4-15×",
    label: "faster builds",
    body: "Demonstrated on LLVM, one of the world's largest C++ codebases.",
  },
  {
    value: "10B+",
    label: "build requests per month",
    body: "served in production.",
  },
  {
    value: "10m",
    label: "to your first cache hit",
    body: "from Docker startup to a reusable remote cache.",
  },
  {
    value: "0",
    label: "build-system rewrites",
    body: "required for Bazel, Buck2, Reclient, Goma, or CMake via recc or BuildStream.",
  },
];

const features = [
  {
    title: "Faster builds. Lower compute bills.",
    body: "Content-addressable storage means unchanged code never compiles twice across your team, your CI, and your agents. Every cache hit is a build you don't pay to run.",
  },
  {
    title: "Scale past one machine. Pay only for what you use.",
    body: "Remote build execution distributes compilation across as many cores as you need and spins them down when you're done. No idle workstations, no idle workers, no idle bill.",
  },
];

const securityFeature = {
  title: "Secure by default.",
  body: "SSO, signed inputs, end-to-end packet integrity. Your source, your artifacts, and your supply chain stay locked down.",
};

const benefits = [
  {
    title: "Written in Rust. Built for scale.",
    body: "Memory-safe, race-free, and no garbage collector to stall your hot path. Over ten billion build requests a month, in production.",
  },
  {
    title: "Ten minutes to your first cache hit.",
    body: "One Docker command. Drops into your existing Bazel, Buck2, Reclient, or CMake setup with zero rewrites.",
  },
  {
    title: "Works with what you've got.",
    body: "C++, Rust, Python, Go, and more. Bazel, Buck2, Reclient, CMake. AWS, GCP, Azure, or your own hardware. No lock-in.",
  },
];

const industries = [
  {
    title: "Robotics & Autonomous Systems",
    body: "The simulation cycle eats compute. NativeLink caches what's stable, distributes what's new, and gets robotics and AV teams from code change to track-tested in minutes instead of hours.",
  },
  {
    title: "Semiconductors & EDA",
    body: "Verification, synthesis, and timing closure devour compute the way C++ builds do and reward content-addressable caching the same way.",
  },
  {
    title: "Consumer Electronics & Mobile",
    body: "Android, embedded firmware, cross-platform clients, and millions of build targets across dozens of variants stay compilable on a coffee break.",
  },
  {
    title: "Developer Infrastructure & OS",
    body: "Linux distributions, security platforms, and the systems other systems depend on need fast, reproducible, and secure infrastructure builds.",
  },
  {
    title: "AI/ML Platforms",
    body: "Model code, data pipelines, and inference services rebuild constantly. NativeLink keeps the loop tight when an AI commits ten times an hour.",
  },
  {
    title: "Browsers & Web Platforms",
    body: "Chromium and its descendants are some of the largest C++ codebases on the open web. NativeLink speaks reclient natively.",
  },
];

export default function HomePage() {
  return (
    <>
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-0 -z-10 bg-brand-glow" />
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[760px] bg-[radial-gradient(ellipse_1100px_600px_at_78%_-5%,rgb(var(--nl-color-brand)/0.18),transparent_70%)]" />
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[600px] bg-dot-grid opacity-50 [mask-image:radial-gradient(ellipse_at_top,black_20%,transparent_70%)]" />

        <Section width="default" className="pt-20 pb-20 md:pt-28">
          <div className="grid items-center gap-14 lg:grid-cols-[1fr_1.1fr]">
            <Reveal>
              <Eyebrow className="mb-5">Build infrastructure for the agentic era</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] text-foreground md:text-[68px]">
                When agents write your code,{" "}
                <span className="bg-gradient-to-r from-brand via-brand to-brand-strong bg-clip-text text-transparent">
                  your build system is the bottleneck.
                </span>
              </h1>
              <p className="mt-6 max-w-[620px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                NativeLink is the parallel compute platform that keeps builds fast while your
                codebase and your agents multiply. Rust-powered, open source, and trusted in
                production by thousands of developers.
              </p>
              <div className="mt-8 flex flex-wrap items-center gap-3">
                <Button size="lg" asChild>
                  <a
                    href="https://github.com/TraceMachina/nativelink"
                    target="_blank"
                    rel="noreferrer"
                  >
                    Clone the repo <span aria-hidden="true">→</span>
                  </a>
                </Button>
                <Button size="lg" variant="outline" asChild>
                  <a href="/docs" target="_blank" rel="noreferrer">
                    Start free
                  </a>
                </Button>
                <Button size="lg" variant="ghost" asChild>
                  <a href="/contact">Talk to us</a>
                </Button>
              </div>
            </Reveal>

            <Reveal delay={0.15}>
              <div className="relative w-full overflow-hidden rounded-2xl border border-border shadow-[0_30px_80px_-25px_rgb(0_0_0_/_0.25)]">
                <iframe
                  className="aspect-video w-full"
                  src="https://www.youtube.com/embed/WLpqFuyLMUQ?si=ZIqaR3taGNgXEyE_"
                  title="YouTube video player"
                  frameBorder="0"
                  allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture; web-share"
                  referrerPolicy="strict-origin-when-cross-origin"
                  allowFullScreen
                />
              </div>
            </Reveal>
          </div>
        </Section>

        <div className="relative border-y border-border/60 bg-surface-elevated/40 py-8">
          <p className="mb-4 text-center font-mono text-[11px] uppercase tracking-[0.18em] text-muted">
            Speaks every major build system
          </p>
          <Marquee speed={45}>
            {integrations.map((name) => (
              <span
                key={name}
                className="font-mono text-xl font-medium tracking-tight text-muted-foreground transition-colors hover:text-brand md:text-2xl"
              >
                {name}
              </span>
            ))}
          </Marquee>
        </div>
      </section>

      <Section width="default" className="border-b border-border/60 py-28">
        <Reveal>
          <div className="mx-auto mb-12 max-w-[760px] text-center">
            <Eyebrow className="mb-4">Built for AI-assisted development</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-[56px]">
              Wire NativeLink into your AI coding agent.
            </h2>
            <p className="mx-auto mt-6 max-w-[640px] text-base leading-relaxed text-muted-foreground md:text-lg">
              The NativeLink MCP server gives Claude Code, Cursor, and Codex five real tools, so
              your agent can configure remote caching, pull docs, and tune builds without leaving
              the editor.
            </p>
          </div>
        </Reveal>

        <Reveal delay={0.08}>
          <div className="mx-auto max-w-[820px]">
            <McpDemo />
          </div>
        </Reveal>
      </Section>

      <Section width="default" className="border-b border-border/60 py-20">
        <Reveal>
          <div className="grid grid-cols-1 gap-x-8 gap-y-10 md:grid-cols-2 lg:grid-cols-4">
            {stats.map((stat) => (
              <div key={stat.label} className="flex flex-col gap-2">
                <div className="font-mono text-4xl font-semibold leading-none tracking-tight text-brand md:text-5xl">
                  {stat.value}
                </div>
                <div className="text-base font-semibold text-foreground">{stat.label}</div>
                <div className="text-sm leading-relaxed text-muted">{stat.body}</div>
              </div>
            ))}
          </div>
        </Reveal>
      </Section>

      <Section width="narrow" className="border-b border-border/60 py-24 text-center">
        <Reveal>
          <blockquote className="text-balance border-l-4 border-foreground pl-6 text-left text-2xl font-semibold leading-[1.3] tracking-[-0.02em] md:text-[36px]">
            "Running NativeLink in production with great results. Great work folks."
          </blockquote>
          <p className="mt-6 font-mono text-sm text-muted-foreground">
            Mustafa Gezen — Rocky Linux
          </p>
        </Reveal>
      </Section>

      <Section width="default" className="border-b border-border/60 pt-16 pb-12">
        <Reveal>
          <p className="mb-10 text-center font-mono text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
            Some companies we are building with
          </p>
          <ul className="flex flex-wrap items-center justify-center gap-x-12 gap-y-8 md:gap-x-16">
            {customerLogos.map((c) => (
              <li key={c.name}>
                <a
                  href={c.href}
                  target="_blank"
                  rel="noopener noreferrer"
                  aria-label={`Visit ${c.name}`}
                  title={c.name}
                  className="inline-flex items-center justify-center text-foreground/65 transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand focus-visible:ring-offset-4 focus-visible:ring-offset-background"
                >
                  <c.Logo className={`${c.className} w-auto text-current`} />
                </a>
              </li>
            ))}
          </ul>
        </Reveal>
      </Section>

      <Section width="default" className="py-28">
        <Reveal>
          <div className="mx-auto mb-14 max-w-[720px] text-center">
            <Eyebrow className="mb-4">What you get</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-[56px]">
              Fast builds without idle infrastructure.
            </h2>
          </div>
        </Reveal>

        <Reveal delay={0.08}>
          <div className="mx-auto mb-12 max-w-[900px]">
            <HeroVisual />
          </div>
        </Reveal>

        <div className="grid gap-5 lg:grid-cols-[1fr_1.25fr]">
          <div className="grid gap-5">
            {features.slice(0, 2).map((feature, i) => (
              <Reveal key={feature.title} delay={i * 0.06}>
                <article className="rounded-2xl border border-border bg-surface p-8">
                  <h3 className="text-2xl font-semibold leading-tight tracking-tight text-foreground">
                    {feature.title}
                  </h3>
                  <p className="mt-4 text-[15px] leading-relaxed text-muted-foreground">
                    {feature.body}
                  </p>
                </article>
              </Reveal>
            ))}
          </div>
          <Reveal delay={0.12}>
            <article className="flex h-full flex-col justify-between rounded-2xl border border-brand/35 bg-brand-soft/40 p-8 shadow-[0_24px_60px_-30px_rgb(var(--nl-color-brand)/0.45)]">
              <Badge variant="brand" className="self-start">
                Secure by default
              </Badge>
              <div>
                <h3 className="mt-8 text-3xl font-semibold leading-tight tracking-tight text-foreground md:text-4xl">
                  {securityFeature.title}
                </h3>
                <p className="mt-5 text-base leading-relaxed text-muted-foreground md:text-lg">
                  {securityFeature.body}
                </p>
              </div>
            </article>
          </Reveal>
        </div>
      </Section>

      <Section width="default" className="border-y border-border/60 bg-surface-elevated/40 py-28">
        <Reveal>
          <div className="mx-auto max-w-[900px] text-center">
            <Eyebrow className="mb-4">
              Programmatic build infrastructure for the agentic era
            </Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-[56px]">
              When agents commit code, the build system is the last honest checkpoint.
            </h2>
            <div className="mx-auto mt-6 max-w-[760px] space-y-4 text-base leading-relaxed text-muted-foreground md:text-lg">
              <p>
                AI agents commit faster than humans can review and pull dependencies humans never
                would. NativeLink turns every build into structured, observable data, with every
                artifact hashed, every dependency traceable, and every action programmable.
              </p>
              <p>
                The substrate your security, compliance, and observability tools have been waiting
                for. At agent speed.
              </p>
            </div>
          </div>
        </Reveal>
        <Reveal delay={0.12}>
          <div className="mx-auto mt-14 max-w-[940px]">
            <ArchitectureDiagram />
          </div>
        </Reveal>
      </Section>

      <Section width="default" className="py-28">
        <Reveal>
          <div className="mb-14 max-w-[680px]">
            <Eyebrow className="mb-4">The NativeLink difference</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              The pieces teams need before builds become the bottleneck.
            </h2>
          </div>
        </Reveal>

        <div className="grid gap-5 lg:grid-cols-3">
          {benefits.map((benefit, i) => (
            <Reveal key={benefit.title} delay={i * 0.06}>
              <article className="h-full rounded-2xl border border-border bg-surface p-8">
                <div className="mb-6 font-mono text-xs text-brand">0{i + 1}</div>
                <h3 className="text-2xl font-semibold leading-tight tracking-tight text-foreground">
                  {benefit.title}
                </h3>
                <p className="mt-4 text-[15px] leading-relaxed text-muted-foreground">
                  {benefit.body}
                </p>
              </article>
            </Reveal>
          ))}
        </div>
      </Section>

      <Section width="default" className="border-t border-border/60 bg-surface-elevated/40 py-28">
        <Reveal>
          <div className="mx-auto mb-14 max-w-[840px] text-center">
            <Eyebrow className="mb-4">
              Built for the codebases that shape the physical world
            </Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              From silicon to simulation, NativeLink runs the builds that ship and the ones agents
              are writing next.
            </h2>
          </div>
        </Reveal>

        <div className="mx-auto flex max-w-[960px] flex-col overflow-hidden rounded-2xl border border-border bg-surface">
          {industries.map((industry, i) => (
            <Reveal key={industry.title} delay={i * 0.04}>
              <details className="group border-t border-border first:border-t-0">
                <summary className="flex cursor-pointer list-none items-center justify-between gap-6 px-6 py-5 text-left transition-colors hover:bg-surface-elevated [&::-webkit-details-marker]:hidden">
                  <h3 className="text-xl font-semibold leading-tight tracking-tight text-foreground">
                    {industry.title}
                  </h3>
                  <span
                    className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full border border-border font-mono text-lg text-muted-foreground transition-transform group-open:rotate-45"
                    aria-hidden="true"
                  >
                    +
                  </span>
                </summary>
                <p className="px-6 pb-6 text-[15px] leading-relaxed text-muted-foreground">
                  {industry.body}
                </p>
              </details>
            </Reveal>
          ))}
        </div>
      </Section>
    </>
  );
}
