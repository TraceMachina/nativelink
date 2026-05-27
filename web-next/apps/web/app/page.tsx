import {
  Badge,
  Button,
  Card,
  CardBody,
  CardHeader,
  CardTitle,
  Eyebrow,
  Reveal,
  Section,
  Stat,
  StatGrid,
} from "@nativelink/ui";
import { TerminalDemo } from "@/components/terminal-demo";

export const metadata = {
  title: "NativeLink — Remote build execution & caching, engineered for scale",
};

const features = [
  {
    title: "Faster builds. Lower compute bills.",
    body: "Cache and parallelize Bazel, Buck2, Reclient, and Pants across your fleet. Cut build wall-time without provisioning a fleet of CI runners.",
  },
  {
    title: "Scale past one machine.",
    body: "Distribute work across hundreds of cores on commodity infra. Pay only for what you use — the scheduler decides.",
  },
  {
    title: "Memory-safe by design.",
    body: "Written in Rust. No garbage collector to stall your hot path. Over a billion build requests a month in production.",
  },
  {
    title: "Open source, batteries included.",
    body: "MIT-licensed. Self-host on your infra in minutes — or let us run it for you. Same engine either way.",
  },
];

const integrations = ["Bazel", "Buck2", "Reclient", "Pants"];

const industries = [
  "Robotics & autonomous systems",
  "Semiconductors & EDA",
  "Consumer electronics & mobile",
  "AI & ML research",
  "Financial services",
  "Life sciences",
];

export default function HomePage() {
  return (
    <>
      {/* HERO */}
      <Section width="default" className="pt-24 pb-20 md:pt-32">
        <div className="grid items-center gap-12 lg:grid-cols-[1.1fr_1fr]">
          <Reveal>
            <Eyebrow className="mb-5">Remote build execution & caching</Eyebrow>
            <h1 className="text-4xl font-bold leading-[1.1] tracking-tight md:text-6xl">
              Builds that finish
              <br />
              before you can blink.
            </h1>
            <p className="mt-6 max-w-[560px] text-lg leading-relaxed text-muted">
              NativeLink caches and parallelizes Bazel, Buck2, Reclient, and Pants at
              infrastructure scale. Written in Rust. Open source. Battle-tested on over
              a billion build requests a month.
            </p>
            <div className="mt-8 flex flex-wrap items-center gap-4">
              <Button size="lg" asChild>
                <a href="/docs">Get started</a>
              </Button>
              <Button size="lg" variant="outline" asChild>
                <a href="/docs">Read the docs</a>
              </Button>
            </div>
            <p className="mt-5 font-mono text-xs text-muted">
              Free forever · Self-host or cloud · No credit card
            </p>
          </Reveal>

          <Reveal delay={0.15}>
            <TerminalDemo
              lines={[
                { comment: true, children: "# 1. Run NativeLink locally" },
                { prompt: true, children: "curl -fsSL nativelink.com/install | sh" },
                { children: "" },
                { comment: true, children: "# 2. Point Bazel at it" },
                {
                  prompt: true,
                  children: "bazel build --remote_cache=grpc://localhost:50051 //...",
                },
                { info: true, children: "INFO: Build completed successfully." },
                { info: true, children: "INFO: Remote cache hits: 432 / 487 (89%)" },
                { info: true, children: "INFO: Elapsed time: 12.4s" },
              ]}
            />
          </Reveal>
        </div>
      </Section>

      {/* STATS */}
      <Section
        width="default"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-16"
      >
        <Reveal>
          <StatGrid>
            <Stat value="1B+" label="Build requests / month" />
            <Stat value="10×" label="Cache hit speedup" />
            <Stat value="<10m" label="To first cache hit" />
            <Stat value="100%" label="Open source core" />
          </StatGrid>
        </Reveal>
      </Section>

      {/* WHY */}
      <Section
        width="default"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-24"
      >
        <Reveal>
          <div className="mx-auto mb-16 max-w-[680px] text-center">
            <Eyebrow className="mb-4">Why NativeLink</Eyebrow>
            <h2 className="text-3xl font-bold leading-[1.15] md:text-5xl">
              Build infrastructure that disappears.
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted md:text-lg">
              The fastest builds are the ones you don't notice. NativeLink runs quietly
              behind your existing build system, doing the work so your engineers don't
              have to wait.
            </p>
          </div>
        </Reveal>

        <div className="grid gap-6 sm:grid-cols-2 lg:grid-cols-4">
          {features.map((f, i) => (
            <Reveal key={f.title} delay={i * 0.05}>
              <Card className="h-full">
                <CardHeader>
                  <CardTitle>{f.title}</CardTitle>
                </CardHeader>
                <CardBody>{f.body}</CardBody>
              </Card>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* INTEGRATIONS */}
      <Section
        width="default"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-24"
      >
        <Reveal>
          <div className="mx-auto mb-12 max-w-[680px] text-center">
            <Eyebrow className="mb-4">Drop-in</Eyebrow>
            <h2 className="text-3xl font-bold leading-[1.15] md:text-5xl">
              Works with your build system.
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted md:text-lg">
              NativeLink speaks the standard remote-execution APIs. If your build system
              supports remote cache or remote execution, it works — no rewrites required.
            </p>
          </div>
        </Reveal>

        <Reveal>
          <ul className="mx-auto grid max-w-[900px] grid-cols-2 gap-px overflow-hidden rounded-md border-2 border-border bg-border sm:grid-cols-4">
            {integrations.map((name) => (
              <li key={name}>
                <a
                  href="/docs"
                  className="flex h-24 items-center justify-center bg-background font-mono text-xl font-semibold transition-colors hover:bg-foreground hover:text-background md:text-2xl"
                >
                  {name}
                </a>
              </li>
            ))}
          </ul>
        </Reveal>
      </Section>

      {/* INDUSTRIES */}
      <Section
        width="default"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-24"
      >
        <div className="grid gap-12 lg:grid-cols-[1fr_1.2fr]">
          <Reveal>
            <Eyebrow className="mb-4">Who uses NativeLink</Eyebrow>
            <h2 className="text-3xl font-bold leading-[1.15] md:text-4xl">
              Built for industries that can't wait.
            </h2>
            <p className="mt-5 max-w-[440px] text-base leading-relaxed text-muted md:text-lg">
              From semiconductor verification to autonomous-vehicle simulation, the
              teams shipping at machine speed run NativeLink under the hood.
            </p>
          </Reveal>
          <Reveal delay={0.1}>
            <ul className="grid grid-cols-1 gap-2 sm:grid-cols-2">
              {industries.map((name) => (
                <li
                  key={name}
                  className="flex items-center gap-3 border-t border-[rgb(var(--nl-color-accent-line))]/40 py-3 font-mono text-base"
                >
                  <span aria-hidden="true" className="text-muted">
                    ▸
                  </span>
                  {name}
                </li>
              ))}
            </ul>
          </Reveal>
        </div>
      </Section>

      {/* QUOTE */}
      <Section
        width="narrow"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-24 text-center"
      >
        <Reveal>
          <Eyebrow className="mb-6">Demonstrated on LLVM</Eyebrow>
          <h3 className="text-balance text-2xl font-semibold leading-[1.3] md:text-4xl">
            "Engineered for the world's largest C++ codebases — and yours."
          </h3>
          <p className="mt-6 font-mono text-sm text-muted">
            Production-tested on LLVM, one of the largest open-source codebases in the world.
          </p>
          <div className="mt-8 flex flex-wrap justify-center gap-3">
            <Badge variant="outline">4× faster builds</Badge>
            <Badge variant="outline">17 min → 4 min</Badge>
            <Badge variant="outline">CMake + recc</Badge>
          </div>
        </Reveal>
      </Section>
    </>
  );
}
