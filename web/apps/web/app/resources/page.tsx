import { Badge, Eyebrow, Reveal, Section } from "@nativelink/ui";
import { getAllPosts } from "../../lib/posts";
import { type PostCardData, PostSection, postToCard } from "./post-sections";

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

// The BazelCon talk lives among the post cards rather than as an
// embedded video so the page stays light and the blog content leads.
const talkCard: PostCardData = {
  key: "talk-hermetic-toolchains",
  href: "https://www.youtube.com/watch?v=uokjTev8myk",
  external: true,
  tags: ["talk"],
  meta: "BazelCon 2024 · 32 min video",
  title: "Hermetic toolchain creation with LRE & Nix",
  excerpt:
    "Aaron Mondal walks through Local Remote Execution — running fully hermetic Bazel builds on your own laptop, no Docker required.",
  cta: "Watch talk",
};

export default function ResourcesPage() {
  const posts = getAllPosts();
  const caseStudies = posts.filter((p) => p.tags.includes("case-studies"));
  const announcements = posts.filter(
    (p) => p.tags.includes("announcements") && !p.tags.includes("case-studies"),
  );
  const others = posts.filter(
    (p) => !p.tags.includes("case-studies") && !p.tags.includes("announcements"),
  );

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

      {/* FROM THE TEAM — full blog listing */}
      <section className="border-t border-border/60 bg-surface-elevated/40">
        <Section width="default" className="pt-24 pb-4">
          <Reveal>
            <div>
              <Eyebrow className="mb-4">Blog</Eyebrow>
              <h2 className="text-balance text-3xl font-semibold leading-[1.1] tracking-[-0.025em] md:text-4xl">
                From the team
              </h2>
            </div>
          </Reveal>
        </Section>

        <PostSection
          title="Case studies"
          cards={caseStudies.map(postToCard)}
          className="pt-12 pb-16"
        />
        <PostSection
          title="Announcements"
          cards={announcements.map(postToCard)}
          className="border-t border-border/60 pt-16 pb-16"
        />
        <PostSection
          title="More from the blog"
          cards={[...others.map(postToCard), talkCard]}
          className="border-t border-border/60 pt-16 pb-28"
        />
      </section>
    </>
  );
}
