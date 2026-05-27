import { Button, Eyebrow, Reveal, Section } from "@nativelink/ui";

export const metadata = { title: "Community" };

const channels = [
  {
    title: "Read the docs",
    body: "Setup guides, architecture deep-dives, and reference for every build system.",
    href: "/docs",
    label: "Open docs",
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
        <path d="M4 4h12a2 2 0 0 1 2 2v14H6a2 2 0 0 1-2-2V4Z" strokeLinejoin="round" />
        <path d="M8 8h6M8 12h6M8 16h4" strokeLinecap="round" />
      </svg>
    ),
  },
  {
    title: "Join the Slack",
    body: "Talk to other operators, share configs, and get help from the core team.",
    href: "https://forms.gle/LtaWSixEC6bYi5xF7",
    label: "Join Slack",
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
        <rect x="3" y="11" width="6" height="2" rx="1" />
        <rect x="11" y="15" width="2" height="6" rx="1" />
        <rect x="15" y="11" width="6" height="2" rx="1" />
        <rect x="11" y="3" width="2" height="6" rx="1" />
      </svg>
    ),
  },
  {
    title: "Clone the repo",
    body: "NativeLink is open source. File issues, send PRs, or star us on GitHub.",
    href: "https://github.com/tracemachina/nativelink",
    label: "View on GitHub",
    icon: (
      <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
        <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.56v-2.01c-3.2.7-3.87-1.36-3.87-1.36-.52-1.34-1.28-1.69-1.28-1.69-1.05-.71.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.76 2.7 1.25 3.36.96.1-.74.4-1.25.73-1.54-2.55-.29-5.23-1.28-5.23-5.68 0-1.25.45-2.28 1.19-3.08-.12-.29-.51-1.46.11-3.04 0 0 .97-.31 3.18 1.18.92-.26 1.91-.39 2.89-.39s1.97.13 2.89.39c2.21-1.49 3.18-1.18 3.18-1.18.62 1.58.23 2.75.11 3.04.74.8 1.19 1.83 1.19 3.08 0 4.41-2.69 5.39-5.25 5.67.41.36.78 1.06.78 2.14v3.17c0 .31.21.68.8.56 4.57-1.52 7.86-5.83 7.86-10.91C23.5 5.65 18.35.5 12 .5Z" />
      </svg>
    ),
  },
];

export default function CommunityPage() {
  return (
    <>
      {/* HERO */}
      <Section width="default" className="pt-24 pb-16 md:pt-32">
        <Reveal>
          <div className="mx-auto max-w-[820px] text-center">
            <Eyebrow className="mb-5">Community</Eyebrow>
            <h1 className="text-balance text-4xl font-bold leading-[1.1] tracking-tight md:text-6xl">
              Join the people building NativeLink.
            </h1>
            <p className="mx-auto mt-6 max-w-[640px] text-lg leading-relaxed text-muted">
              Operators, contributors, and the engineers who build NativeLink — all in
              the same room.
            </p>
          </div>
        </Reveal>
      </Section>

      {/* CHANNELS */}
      <Section width="default" className="pb-24">
        <div className="grid gap-6 md:grid-cols-3">
          {channels.map((c, i) => (
            <Reveal key={c.title} delay={i * 0.05}>
              <a
                href={c.href}
                target={c.href.startsWith("http") ? "_blank" : undefined}
                rel={c.href.startsWith("http") ? "noreferrer" : undefined}
                className="group flex h-full flex-col rounded-md border-2 border-border bg-surface/50 p-8 transition-colors hover:border-foreground hover:bg-foreground hover:text-background"
              >
                <span className="mb-6 inline-flex h-12 w-12 items-center justify-center [&_svg]:h-7 [&_svg]:w-7">
                  {c.icon}
                </span>
                <h3 className="text-xl font-semibold leading-tight">{c.title}</h3>
                <p className="mt-3 text-base leading-relaxed text-muted-foreground group-hover:text-background/80">
                  {c.body}
                </p>
                <span className="mt-6 inline-flex items-center gap-1 font-mono text-sm">
                  {c.label} <span aria-hidden="true">→</span>
                </span>
              </a>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* CONTRIBUTORS */}
      <Section
        width="narrow"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-24 text-center"
      >
        <Reveal>
          <Eyebrow className="mb-5">Thank you</Eyebrow>
          <h2 className="text-balance text-3xl font-bold leading-[1.15] md:text-5xl">
            We love our contributors.
          </h2>
          <p className="mx-auto mt-6 max-w-[600px] text-base leading-relaxed text-muted md:text-lg">
            As a small token of appreciation, we'd love to send something special to
            active NativeLink community members and contributors. Fill out the form
            and our team will be in touch.
          </p>
          <div className="mt-8 flex justify-center">
            <Button size="lg" asChild>
              <a href="https://forms.gle/LtaWSixEC6bYi5xF7" target="_blank" rel="noreferrer">
                Fill out the form
              </a>
            </Button>
          </div>
        </Reveal>
      </Section>
    </>
  );
}
