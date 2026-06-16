import {
  Badge,
  Button,
  Eyebrow,
  Reveal,
  Section,
  YouTubeEmbed,
  cn,
} from "@nativelink/ui";

// (Badge is used in featured + announcement cards below.)

export const metadata = { title: "Resources" };

const featuredPost = {
  tag: "Case study",
  title: "How LLVM cut clang compile time from 17 minutes to 4 with NativeLink + recc.",
  excerpt:
    "LLVM contributors needed faster iteration without rewriting their CMake setup. They pointed recc at a NativeLink cluster and saw 4× speedups on first try.",
  date: "March 14, 2026",
  readingTime: "8 min read",
  href: "https://reidkleckner.dev/posts/llvm-recc-nativelink/",
};

const announcements = [
  {
    tag: "Announcement",
    title: "NativeLink v1.3 — Persistent workers, faster scheduling, GPU support.",
    excerpt: "The biggest release this quarter. Workers stay warm between actions, the scheduler dispatches 3× faster, and we now ship first-class CUDA/ROCm support.",
    date: "May 02, 2026",
    readingTime: "5 min",
    accent: "brand" as const,
  },
  {
    tag: "Talk",
    title: "Hermetic toolchain creation with LRE & Nix",
    excerpt: "Aaron Mondal walks through Local Remote Execution — running fully hermetic Bazel builds on your own laptop, no Docker required.",
    date: "April 18, 2026",
    readingTime: "32 min video",
    accent: "default" as const,
  },
  {
    tag: "Announcement",
    title: "Trace Machina raises $15M to build the Bazel-grade cache for everyone.",
    excerpt: "We're hiring across systems, distributed storage, and developer experience.",
    date: "March 28, 2026",
    readingTime: "3 min",
    accent: "default" as const,
  },
];

const caseStudies = [
  {
    company: "Samsung Internet",
    tagline: "Browser builds, 6× faster.",
    summary: "Samsung's Chromium-based browser team adopted NativeLink to cut their per-commit CI wall-time from 38 minutes to under 7.",
    metric: "6×",
  },
  {
    company: "Aurora Robotics",
    tagline: "Simulation that keeps up with the road.",
    summary: "Replay-driven sim that used to chew through nightly fleets now finishes during code review — and the cache survives every rebase.",
    metric: "94%",
  },
  {
    company: "Cirque Semi",
    tagline: "Verification across 4,000 cores.",
    summary: "An EDA shop with a notoriously hot Verilog regression suite now hands every job to NativeLink's scheduler. Tape-out velocity 2.3× higher.",
    metric: "2.3×",
  },
];

const blogPosts = [
  {
    tag: "Engineering",
    title: "Why we rewrote our scheduler in async Rust.",
    excerpt: "How we shaved p99 latency from 24ms to 1.8ms by ditching threads, embracing tokio, and being honest about lock contention.",
    date: "Apr 21",
  },
  {
    tag: "Deep dive",
    title: "Content-addressed storage at a billion requests a month.",
    excerpt: "Sharding strategy, hot-key handling, and the surprising thing that happens when half your fleet asks for the same blob at once.",
    date: "Apr 04",
  },
  {
    tag: "Tutorial",
    title: "Migrate a Bazel monorepo to remote execution in an afternoon.",
    excerpt: "A step-by-step playbook with real configs, common pitfalls, and the diff between a 12-minute build and an 80-second one.",
    date: "Mar 19",
  },
  {
    tag: "Engineering",
    title: "Hermetic builds without Docker — how LRE works.",
    excerpt: "Nix profiles + content-addressed inputs let you reproduce CI exactly on your laptop. No containers, no Dockerfiles, no surprises.",
    date: "Mar 05",
  },
];

export default function ResourcesPage() {
  return (
    <>
      {/* HERO */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[500px] bg-[radial-gradient(ellipse_900px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.13),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-16 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[820px] text-center">
              <Eyebrow className="mb-5">Resources</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] md:text-[68px]">
                Deep-tech writing for engineers who{" "}
                <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
                  build the builds
                </span>
                .
              </h1>
              <p className="mx-auto mt-6 max-w-[640px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                Case studies, conference talks, and write-ups from the team building
                NativeLink — plus highlights from the broader community.
              </p>
            </div>
          </Reveal>
        </Section>
      </section>

      {/* FEATURED POST */}
      <Section width="default" className="pb-20">
        <Reveal>
          <a
            href={featuredPost.href}
            target="_blank"
            rel="noreferrer"
            className="group block overflow-hidden rounded-3xl border border-border bg-surface transition-all hover:border-brand/40 hover:shadow-[0_30px_80px_-30px_rgb(var(--nl-color-brand)/0.35)]"
          >
            <div className="grid lg:grid-cols-[1.1fr_1fr]">
              <div className="flex flex-col justify-between p-10 md:p-12">
                <div>
                  <div className="mb-5 flex items-center gap-3">
                    <Badge variant="brand">{featuredPost.tag}</Badge>
                    <span className="font-mono text-xs text-muted">
                      {featuredPost.date} · {featuredPost.readingTime}
                    </span>
                  </div>
                  <h2 className="text-balance text-3xl font-semibold leading-[1.15] tracking-[-0.02em] text-foreground md:text-4xl">
                    {featuredPost.title}
                  </h2>
                  <p className="mt-5 text-base leading-relaxed text-muted-foreground md:text-lg">
                    {featuredPost.excerpt}
                  </p>
                </div>
                <div className="mt-8 inline-flex items-center gap-1.5 font-mono text-sm text-brand">
                  Read the write-up{" "}
                  <span aria-hidden="true" className="transition-transform group-hover:translate-x-1">
                    →
                  </span>
                </div>
              </div>
              <div className="relative min-h-[280px] overflow-hidden bg-gradient-to-br from-brand-soft via-brand-soft/40 to-transparent">
                <div className="pointer-events-none absolute inset-0 bg-dot-grid opacity-40" />
                <div className="absolute inset-0 flex items-center justify-center p-12">
                  <div className="flex flex-col items-center gap-4 text-center">
                    <div className="font-mono text-[10px] uppercase tracking-[0.2em] text-muted-foreground">
                      Build wall-time
                    </div>
                    <div className="flex items-baseline gap-4">
                      <div>
                        <div className="font-mono text-4xl font-semibold text-muted-foreground line-through decoration-foreground/30 md:text-5xl">
                          17m
                        </div>
                        <div className="mt-1 text-xs text-muted">Before</div>
                      </div>
                      <div className="font-mono text-2xl text-muted">→</div>
                      <div>
                        <div className="bg-gradient-to-br from-brand to-brand-strong bg-clip-text font-mono text-5xl font-semibold text-transparent md:text-6xl">
                          4m
                        </div>
                        <div className="mt-1 text-xs text-brand">With NativeLink</div>
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </a>
        </Reveal>
      </Section>

      {/* ANNOUNCEMENTS + VIDEO */}
      <Section width="default" className="border-t border-border/60 bg-surface-elevated/40 py-24">
        <Reveal>
          <div className="mb-12 flex items-end justify-between">
            <div>
              <Eyebrow className="mb-4">Announcements</Eyebrow>
              <h2 className="text-balance text-3xl font-semibold leading-[1.1] tracking-[-0.025em] md:text-4xl">
                From the team
              </h2>
            </div>
            <a href="#" className="hidden font-mono text-sm text-brand md:inline-flex">
              All posts →
            </a>
          </div>
        </Reveal>

        <div className="grid gap-8 lg:grid-cols-[1.2fr_1fr]">
          <Reveal>
            <div className="overflow-hidden rounded-2xl">
              <YouTubeEmbed
                id="uokjTev8myk"
                title="Hermetic Toolchain Creation with Local Remote Execution (LRE) & Nix"
              />
            </div>
            <p className="mt-4 text-base leading-relaxed text-foreground">
              <span className="font-mono text-xs uppercase tracking-widest text-muted">
                Featured talk ·{" "}
              </span>
              Hermetic Toolchain Creation with Local Remote Execution & Nix
            </p>
            <p className="mt-1 text-sm text-muted-foreground">
              Aaron Mondal, Trace Machina — 32 min
            </p>
          </Reveal>

          <div className="flex flex-col gap-3">
            {announcements.map((p, i) => (
              <Reveal key={p.title} delay={i * 0.05}>
                <a
                  href="#"
                  className={cn(
                    "group block rounded-2xl border p-6 transition-all hover:-translate-y-0.5",
                    p.accent === "brand"
                      ? "border-brand/40 bg-brand-soft/30 hover:border-brand"
                      : "border-border bg-surface hover:border-border-strong",
                  )}
                >
                  <div className="mb-3 flex items-center gap-2">
                    <Badge variant={p.accent === "brand" ? "brand" : "default"}>
                      {p.tag}
                    </Badge>
                    <span className="font-mono text-xs text-muted">
                      {p.date} · {p.readingTime}
                    </span>
                  </div>
                  <h3 className="text-lg font-semibold leading-tight tracking-tight text-foreground">
                    {p.title}
                  </h3>
                  <p className="mt-2 text-sm leading-relaxed text-muted-foreground">
                    {p.excerpt}
                  </p>
                </a>
              </Reveal>
            ))}
          </div>
        </div>
      </Section>

      {/* CASE STUDIES */}
      <Section width="default" className="border-t border-border/60 py-28">
        <Reveal>
          <div className="mb-14 flex flex-col gap-6 md:flex-row md:items-end md:justify-between">
            <div>
              <Eyebrow className="mb-4">Case studies</Eyebrow>
              <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
                In production.
              </h2>
            </div>
            <p className="max-w-[420px] text-base leading-relaxed text-muted-foreground">
              How teams are using NativeLink to keep their build farms — and their
              engineers — moving at full speed.
            </p>
          </div>
        </Reveal>

        <div className="grid gap-5 md:grid-cols-3">
          {caseStudies.map((cs, i) => (
            <Reveal key={cs.company} delay={i * 0.06}>
              <article className="group h-full overflow-hidden rounded-2xl border border-border bg-surface p-7 transition-all hover:border-brand/40">
                <div className="flex items-start justify-between">
                  <Eyebrow tone="muted" className="text-[10px]">
                    {cs.company}
                  </Eyebrow>
                  <div className="bg-gradient-to-br from-brand to-brand-strong bg-clip-text font-mono text-3xl font-semibold text-transparent">
                    {cs.metric}
                  </div>
                </div>
                <h3 className="mt-4 text-xl font-semibold leading-tight tracking-tight text-foreground">
                  {cs.tagline}
                </h3>
                <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                  {cs.summary}
                </p>
                <div className="mt-6 inline-flex items-center gap-1.5 font-mono text-sm text-brand opacity-0 transition-opacity group-hover:opacity-100">
                  Read case study <span aria-hidden="true">→</span>
                </div>
              </article>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* BLOG */}
      <Section width="default" className="border-t border-border/60 bg-surface-elevated/40 py-24">
        <Reveal>
          <div className="mb-12 flex items-end justify-between">
            <div>
              <Eyebrow className="mb-4">Blog</Eyebrow>
              <h2 className="text-balance text-3xl font-semibold leading-[1.1] tracking-[-0.025em] md:text-4xl">
                Recent writing
              </h2>
            </div>
            <Button asChild variant="outline" size="sm">
              <a href="#">All posts</a>
            </Button>
          </div>
        </Reveal>

        <div className="grid gap-5 md:grid-cols-2">
          {blogPosts.map((p, i) => (
            <Reveal key={p.title} delay={i * 0.04}>
              <a
                href="#"
                className="group flex h-full flex-col justify-between rounded-2xl border border-border bg-surface p-7 transition-all hover:border-brand/40 hover:shadow-[0_20px_50px_-25px_rgb(var(--nl-color-brand)/0.35)]"
              >
                <div>
                  <div className="mb-4 flex items-center gap-2">
                    <Badge variant="outline">{p.tag}</Badge>
                    <span className="font-mono text-xs text-muted">{p.date}</span>
                  </div>
                  <h3 className="text-xl font-semibold leading-tight tracking-tight text-foreground">
                    {p.title}
                  </h3>
                  <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                    {p.excerpt}
                  </p>
                </div>
                <div className="mt-6 inline-flex items-center gap-1.5 font-mono text-sm text-brand transition-all">
                  Read post{" "}
                  <span aria-hidden="true" className="transition-transform group-hover:translate-x-1">
                    →
                  </span>
                </div>
              </a>
            </Reveal>
          ))}
        </div>
      </Section>
    </>
  );
}
