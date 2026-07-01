import { Badge, Eyebrow, Reveal, Section, YouTubeEmbed, cn } from "@nativelink/ui";
import Image from "next/image";
import Link from "next/link";

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

const caseStudies = [
  {
    slug: "case-study-ciq",
    title: "Case study: CIQ",
    excerpt:
      "The Rocky Linux founding sponsor moved off physical-disk block storage onto NativeLink, cutting average PR CI time from 15 minutes to 3 and deploys from over an hour to 15 minutes.",
    image: "https://nativelink-cdn.s3.us-east-1.amazonaws.com/nativelink_ciq.webp",
  },
  {
    slug: "case-study-fortune-50-manufacturer",
    title: "Fortune 50 manufacturer: 20× faster builds, 30× denser storage",
    excerpt:
      "A Fortune 50 manufacturer building a Chromium-based browser took build times from 2 days to 2 hours and reached a 30× increase in CAS storage density.",
    metric: "20×",
    metricLabel: "Build speed",
  },
  {
    slug: "case-study-last-mile-ai",
    title: "Case study: LastMile AI",
    excerpt:
      "The team behind mcp-agent uses NativeLink as a remote Bazel cache to eliminate repetitive local rebuilds and fold heavy dependencies like Temporal and Envoy into the build.",
    image: "https://nativelink-cdn.s3.us-east-1.amazonaws.com/lastmileai-logo.webp",
  },
  {
    slug: "case-study-thirdwave-automation",
    title: "Thirdwave Automation and NativeLink",
    excerpt:
      "The autonomous-forklift robotics company cut build times by 80% and cloud costs by 50%, running up to 50K simultaneous jobs while testing daily instead of weekly.",
  },
];

const announcements = [
  {
    tag: "Announcement",
    title: "NativeLink — the open-source RBE + CAS platform in Rust",
    excerpt:
      "We released NativeLink: a Rust implementation of Bazel's RBE protocol and Content Addressable Storage that integrates with Bazel, Reclient, Goma, and Buck2 — at no cost, on every major OS.",
    date: "September 17, 2024",
    readingTime: "30 sec read",
    href: "/resources/blog/nativelink-open-source",
    accent: "brand",
  },
  {
    tag: "Announcement",
    title: "TraceMachina: Seed Funding",
    excerpt:
      "Trace Machina came out of stealth with $4.7M in seed funding to build simulation infrastructure for safety-critical technologies.",
    date: "September 10, 2024",
    readingTime: "30 sec read",
    href: "/resources/blog/tracemachina-seedfunding",
    accent: "default",
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
                Case studies, conference talks, and write-ups from the team building NativeLink —
                plus highlights from the broader community.
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
                  <span
                    aria-hidden="true"
                    className="transition-transform group-hover:translate-x-1"
                  >
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

      {/* CASE STUDIES */}
      <Section width="default" className="border-t border-border/60 pb-24 pt-20">
        <Reveal>
          <div className="mb-12">
            <Eyebrow className="mb-4">Case studies</Eyebrow>
            <h2 className="text-balance text-3xl font-semibold leading-[1.1] tracking-[-0.025em] md:text-4xl">
              Teams shipping faster on NativeLink
            </h2>
          </div>
        </Reveal>

        <div className="grid gap-6 md:grid-cols-2">
          {caseStudies.map((c, i) => (
            <Reveal key={c.slug} delay={i * 0.05}>
              <Link
                href={`/resources/blog/${c.slug}`}
                className="group flex h-full flex-col overflow-hidden rounded-2xl border border-border bg-surface transition-all hover:-translate-y-0.5 hover:border-brand/40 hover:shadow-[0_24px_60px_-30px_rgb(var(--nl-color-brand)/0.35)]"
              >
                <div className="relative h-44 overflow-hidden bg-gradient-to-br from-brand-soft via-brand-soft/40 to-transparent">
                  <div className="pointer-events-none absolute inset-0 bg-dot-grid opacity-40" />
                  {c.image ? (
                    <Image
                      src={c.image}
                      alt={c.title}
                      fill
                      sizes="(min-width: 768px) 50vw, 100vw"
                      className="object-contain p-10"
                    />
                  ) : c.metric ? (
                    <div className="absolute inset-0 flex flex-col items-center justify-center">
                      <div className="bg-gradient-to-br from-brand to-brand-strong bg-clip-text font-mono text-6xl font-semibold text-transparent">
                        {c.metric}
                      </div>
                      <div className="mt-1 font-mono text-[10px] uppercase tracking-[0.2em] text-muted-foreground">
                        {c.metricLabel}
                      </div>
                    </div>
                  ) : (
                    <div className="absolute inset-0 flex items-center justify-center">
                      <span className="font-mono text-xs uppercase tracking-[0.2em] text-muted-foreground">
                        Case study
                      </span>
                    </div>
                  )}
                </div>
                <div className="flex flex-1 flex-col p-7">
                  <div className="mb-3">
                    <Badge variant="brand">Case study</Badge>
                  </div>
                  <h3 className="text-xl font-semibold leading-tight tracking-tight text-foreground">
                    {c.title}
                  </h3>
                  <p className="mt-3 text-sm leading-relaxed text-muted-foreground">{c.excerpt}</p>
                  <div className="mt-5 inline-flex items-center gap-1.5 font-mono text-sm text-brand">
                    Read case study{" "}
                    <span
                      aria-hidden="true"
                      className="transition-transform group-hover:translate-x-1"
                    >
                      →
                    </span>
                  </div>
                </div>
              </Link>
            </Reveal>
          ))}
        </div>
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
                <Link
                  href={p.href}
                  className={cn(
                    "group block rounded-2xl border p-6 transition-all hover:-translate-y-0.5",
                    p.accent === "brand"
                      ? "border-brand/40 bg-brand-soft/30 hover:border-brand"
                      : "border-border bg-surface hover:border-border-strong",
                  )}
                >
                  <div className="mb-3 flex items-center gap-2">
                    <Badge variant={p.accent === "brand" ? "brand" : "default"}>{p.tag}</Badge>
                    <span className="font-mono text-xs text-muted">
                      {p.date} · {p.readingTime}
                    </span>
                  </div>
                  <h3 className="text-lg font-semibold leading-tight tracking-tight text-foreground">
                    {p.title}
                  </h3>
                  <p className="mt-2 text-sm leading-relaxed text-muted-foreground">{p.excerpt}</p>
                </Link>
              </Reveal>
            ))}
          </div>
        </div>
      </Section>
    </>
  );
}
