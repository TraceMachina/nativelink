import { Badge, Button, Counter, Eyebrow, Reveal, Section } from "@nativelink/ui";
import { getContributors, getRepoStats } from "@/lib/github";

export const metadata = { title: "Community" };

const channels = [
  {
    title: "Read the docs",
    body: "Setup guides, architecture deep-dives, and reference for every supported build system.",
    href: "/docs",
    label: "Open docs",
    accent: "default" as const,
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
        <path d="M4 4h10a2 2 0 0 1 2 2v14H6a2 2 0 0 1-2-2V4Z M16 6h4v14h-12" strokeLinejoin="round" />
        <path d="M8 9h6M8 13h6M8 17h4" strokeLinecap="round" />
      </svg>
    ),
  },
  {
    title: "Join the Slack",
    body: "Talk to operators, share configs, and get help from the core team within hours, not days.",
    href: "https://forms.gle/LtaWSixEC6bYi5xF7",
    label: "Join Slack",
    accent: "brand" as const,
    icon: (
      <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
        <rect x="3" y="11" width="6" height="2" rx="1" />
        <rect x="11" y="15" width="2" height="6" rx="1" />
        <rect x="15" y="11" width="6" height="2" rx="1" />
        <rect x="11" y="3" width="2" height="6" rx="1" />
        <path d="M9 9h2v2H9zM13 13h2v2h-2z" />
      </svg>
    ),
  },
  {
    title: "Clone the repo",
    body: "MIT-licensed. File issues, send PRs, or just star us. Every contribution gets a review.",
    href: "https://github.com/tracemachina/nativelink",
    label: "View on GitHub",
    accent: "default" as const,
    icon: (
      <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
        <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.56v-2.01c-3.2.7-3.87-1.36-3.87-1.36-.52-1.34-1.28-1.69-1.28-1.69-1.05-.71.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.76 2.7 1.25 3.36.96.1-.74.4-1.25.73-1.54-2.55-.29-5.23-1.28-5.23-5.68 0-1.25.45-2.28 1.19-3.08-.12-.29-.51-1.46.11-3.04 0 0 .97-.31 3.18 1.18.92-.26 1.91-.39 2.89-.39s1.97.13 2.89.39c2.21-1.49 3.18-1.18 3.18-1.18.62 1.58.23 2.75.11 3.04.74.8 1.19 1.83 1.19 3.08 0 4.41-2.69 5.39-5.25 5.67.41.36.78 1.06.78 2.14v3.17c0 .31.21.68.8.56 4.57-1.52 7.86-5.83 7.86-10.91C23.5 5.65 18.35.5 12 .5Z" />
      </svg>
    ),
  },
];

export default async function CommunityPage() {
  const [{ contributors, total }, stats] = await Promise.all([
    getContributors(),
    getRepoStats(),
  ]);
  const starsK = stats.stars / 1000;

  return (
    <>
      {/* HERO */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[500px] bg-[radial-gradient(ellipse_900px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.13),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-16 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[820px] text-center">
              <Eyebrow className="mb-5">Community</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] md:text-[68px]">
                Join the people{" "}
                <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
                  building NativeLink
                </span>
                .
              </h1>
              <p className="mx-auto mt-6 max-w-[600px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                Operators, contributors, and the engineers who build NativeLink — all in
                the same room.
              </p>
            </div>
          </Reveal>
        </Section>
      </section>

      {/* GITHUB STATS BAR */}
      <Section width="default" className="border-y border-border/60 bg-surface-elevated/40 py-10">
        <Reveal>
          <div className="grid grid-cols-2 gap-y-6 text-center md:grid-cols-4">
            <div>
              <div className="font-mono text-3xl font-semibold tracking-tight text-foreground md:text-4xl">
                {starsK >= 1 ? (
                  <Counter to={starsK} decimals={1} suffix="k" />
                ) : (
                  <Counter to={stats.stars} />
                )}
              </div>
              <div className="mt-1 text-xs text-muted">GitHub stars</div>
            </div>
            <div>
              <div className="font-mono text-3xl font-semibold tracking-tight text-brand md:text-4xl">
                <Counter to={total} />
              </div>
              <div className="mt-1 text-xs text-muted">Contributors</div>
            </div>
            <div>
              <div className="font-mono text-3xl font-semibold tracking-tight text-foreground md:text-4xl">
                <Counter to={stats.forks} />
              </div>
              <div className="mt-1 text-xs text-muted">Forks</div>
            </div>
            <div>
              <div className="font-mono text-3xl font-semibold tracking-tight text-foreground md:text-4xl">
                <Counter to={stats.openIssues} />
              </div>
              <div className="mt-1 text-xs text-muted">Open issues</div>
            </div>
          </div>
        </Reveal>
      </Section>

      {/* CHANNELS */}
      <Section width="default" className="py-24">
        <div className="grid gap-5 md:grid-cols-3">
          {channels.map((c, i) => (
            <Reveal key={c.title} delay={i * 0.05}>
              <a
                href={c.href}
                target={c.href.startsWith("http") ? "_blank" : undefined}
                rel={c.href.startsWith("http") ? "noreferrer" : undefined}
                className={`group relative flex h-full flex-col overflow-hidden rounded-2xl border p-8 transition-all hover:-translate-y-1 ${
                  c.accent === "brand"
                    ? "border-brand/40 bg-brand-soft/30 hover:border-brand hover:shadow-[0_30px_80px_-30px_rgb(var(--nl-color-brand)/0.5)]"
                    : "border-border bg-surface hover:border-border-strong"
                }`}
              >
                <span
                  className={`mb-8 inline-flex h-12 w-12 items-center justify-center rounded-xl ${
                    c.accent === "brand"
                      ? "bg-brand text-brand-foreground"
                      : "bg-brand-soft text-brand"
                  } [&_svg]:h-6 [&_svg]:w-6`}
                >
                  {c.icon}
                </span>
                <h3 className="text-2xl font-semibold leading-tight tracking-tight text-foreground">
                  {c.title}
                </h3>
                <p className="mt-3 flex-1 text-[15px] leading-relaxed text-muted-foreground">
                  {c.body}
                </p>
                <span className="mt-8 inline-flex items-center gap-1.5 font-mono text-sm text-brand">
                  {c.label}{" "}
                  <span aria-hidden="true" className="transition-transform group-hover:translate-x-1">
                    →
                  </span>
                </span>
              </a>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* CONTRIBUTORS */}
      <Section width="default" className="border-t border-border/60 py-24">
        <Reveal>
          <div className="mx-auto mb-12 max-w-[680px] text-center">
            <Eyebrow className="mb-4">Thank you</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              We love our{" "}
              <span className="text-brand">{total} contributors</span>.
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted-foreground md:text-lg">
              From kernel-level optimizations to typo fixes — every PR gets reviewed
              and credited. Pulled live from{" "}
              <a
                href="https://github.com/TraceMachina/nativelink/graphs/contributors"
                target="_blank"
                rel="noreferrer"
                className="text-brand underline-offset-4 hover:underline"
              >
                GitHub
              </a>
              .
            </p>
          </div>
        </Reveal>

        <Reveal>
          <div className="mx-auto max-w-[920px]">
            <div className="flex flex-wrap justify-center gap-3">
              {contributors.slice(0, 24).map((c) => (
                <a
                  key={c.login}
                  href={c.html_url}
                  target="_blank"
                  rel="noreferrer"
                  title={`${c.login} — ${c.contributions} contributions`}
                  className="group relative overflow-hidden rounded-full border-2 border-border bg-surface transition-all hover:-translate-y-0.5 hover:border-brand"
                >
                  <img
                    src={c.avatar_url}
                    alt={c.login}
                    width={56}
                    height={56}
                    loading="lazy"
                    className="block h-14 w-14"
                  />
                </a>
              ))}
              {total > 24 ? (
                <a
                  href="https://github.com/TraceMachina/nativelink/graphs/contributors"
                  target="_blank"
                  rel="noreferrer"
                  className="flex h-14 w-14 items-center justify-center rounded-full border-2 border-dashed border-border-strong font-mono text-xs text-muted-foreground transition-colors hover:border-brand hover:text-brand"
                >
                  +{total - 24}
                </a>
              ) : null}
            </div>

            <div className="mt-14 rounded-2xl border border-border bg-surface p-8 text-center">
              <Badge variant="brand" className="mb-4">
                Contributor program
              </Badge>
              <h3 className="text-2xl font-semibold tracking-tight">
                Send us your handle, we'll send you swag.
              </h3>
              <p className="mx-auto mt-3 max-w-[480px] text-[15px] text-muted-foreground">
                Merged a PR? Helped someone on Slack? Wrote a tutorial? Fill out the
                form and our team will be in touch.
              </p>
              <div className="mt-6 flex justify-center">
                <Button size="lg" asChild>
                  <a
                    href="https://forms.gle/LtaWSixEC6bYi5xF7"
                    target="_blank"
                    rel="noreferrer"
                  >
                    Fill out the form
                  </a>
                </Button>
              </div>
            </div>
          </div>
        </Reveal>
      </Section>
    </>
  );
}
