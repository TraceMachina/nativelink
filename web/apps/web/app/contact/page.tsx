import { Eyebrow, Reveal, Section } from "@nativelink/ui";
import { ContactForm } from "./contact-form";

export const metadata = {
  title: "Contact",
  description: "Get in touch with the team behind NativeLink.",
};

const channels = [
  {
    label: "Sales",
    title: "hello@nativelink.com",
    body: "Talk to a solutions engineer about your team's build farm.",
    href: "mailto:hello@nativelink.com",
    icon: <path d="M4 6 h16 a2 2 0 0 1 2 2 v8 a2 2 0 0 1 -2 2 H4 a2 2 0 0 1 -2 -2 V8 a2 2 0 0 1 2 -2 Z M4 8 l8 6 l8 -6" strokeLinejoin="round" />,
  },
  {
    label: "General",
    title: "contact@nativelink.com",
    body: "Questions, comments, or anything else.",
    href: "mailto:contact@nativelink.com",
    icon: <><circle cx="12" cy="12" r="9" /><path d="M9 9 a3 3 0 0 1 6 1 c0 2-3 3-3 4 M12 17 v.01" strokeLinecap="round" /></>,
  },
  {
    label: "Security",
    title: "security@nativelink.com",
    body: "Responsible-disclosure reports. We triage within 24 h.",
    href: "mailto:security@nativelink.com",
    icon: <path d="M12 2 L4 6 V13 C4 17 8 20 12 22 C16 20 20 17 20 13 V6 L12 2 Z" strokeLinejoin="round" />,
  },
];

const socialLinks = [
  {
    label: "GitHub",
    handle: "TraceMachina/nativelink",
    href: "https://github.com/TraceMachina/nativelink",
    icon: (
      <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.56v-2.01c-3.2.7-3.87-1.36-3.87-1.36-.52-1.34-1.28-1.69-1.28-1.69-1.05-.71.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.76 2.7 1.25 3.36.96.1-.74.4-1.25.73-1.54-2.55-.29-5.23-1.28-5.23-5.68 0-1.25.45-2.28 1.19-3.08-.12-.29-.51-1.46.11-3.04 0 0 .97-.31 3.18 1.18.92-.26 1.91-.39 2.89-.39s1.97.13 2.89.39c2.21-1.49 3.18-1.18 3.18-1.18.62 1.58.23 2.75.11 3.04.74.8 1.19 1.83 1.19 3.08 0 4.41-2.69 5.39-5.25 5.67.41.36.78 1.06.78 2.14v3.17c0 .31.21.68.8.56 4.57-1.52 7.86-5.83 7.86-10.91C23.5 5.65 18.35.5 12 .5Z" />
    ),
    fill: true,
  },
  {
    label: "Slack",
    handle: "Join the workspace",
    href: "https://forms.gle/LtaWSixEC6bYi5xF7",
    icon: (
      <>
        <rect x="3" y="11" width="6" height="2" rx="1" />
        <rect x="11" y="15" width="2" height="6" rx="1" />
        <rect x="15" y="11" width="6" height="2" rx="1" />
        <rect x="11" y="3" width="2" height="6" rx="1" />
        <path d="M9 9h2v2H9zM13 13h2v2h-2z" />
      </>
    ),
    fill: true,
  },
  {
    label: "Office",
    handle: "548 Market St, San Francisco, CA 94104",
    href: "https://maps.google.com/?q=548+Market+St+San+Francisco",
    icon: (
      <>
        <path d="M12 22s8-7.5 8-13a8 8 0 1 0-16 0c0 5.5 8 13 8 13Z" />
        <circle cx="12" cy="9" r="3" />
      </>
    ),
    fill: false,
  },
];

export default function ContactPage() {
  return (
    <>
      {/* HERO */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[500px] bg-[radial-gradient(ellipse_900px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.16),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-12 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[820px] text-center">
              <Eyebrow className="mb-5">Contact</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] md:text-[64px]">
                Talk to the people who{" "}
                <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
                  build the builds
                </span>
                .
              </h1>
              <p className="mx-auto mt-6 max-w-[600px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                Real engineers reply, not a sales-bot tree. Pick a channel, drop us
                a note, or fill out the form — we usually answer within a few hours.
              </p>
            </div>
          </Reveal>
        </Section>
      </section>

      {/* DIRECT CHANNELS */}
      <Section width="default" className="pb-16">
        <Reveal>
          <div className="grid gap-4 md:grid-cols-3">
            {channels.map((c) => (
              <a
                key={c.label}
                href={c.href}
                className="group flex h-full flex-col rounded-2xl border border-border bg-surface p-6 transition-all hover:-translate-y-0.5 hover:border-brand/40 hover:shadow-[0_20px_50px_-30px_rgb(var(--nl-color-brand)/0.4)]"
              >
                <span className="mb-5 inline-flex h-10 w-10 items-center justify-center rounded-xl bg-brand-soft text-brand transition-colors group-hover:bg-brand group-hover:text-brand-foreground">
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
                    {c.icon}
                  </svg>
                </span>
                <p className="font-mono text-[10px] uppercase tracking-[0.18em] text-muted">
                  {c.label}
                </p>
                <p className="mt-2 font-mono text-sm font-semibold text-foreground">
                  {c.title}
                </p>
                <p className="mt-2 flex-1 text-sm leading-relaxed text-muted-foreground">
                  {c.body}
                </p>
              </a>
            ))}
          </div>
        </Reveal>
      </Section>

      {/* FORM + SOCIAL */}
      <Section width="default" className="border-t border-border/60 py-20">
        <div className="grid gap-12 lg:grid-cols-[1.4fr_1fr] lg:gap-16">
          <Reveal>
            <Eyebrow className="mb-4">Or write to us</Eyebrow>
            <h2 className="text-balance text-3xl font-semibold leading-[1.1] tracking-[-0.025em] md:text-4xl">
              Tell us what you're building.
            </h2>
            <p className="mt-4 text-base leading-relaxed text-muted-foreground md:text-lg">
              We'll route your note to the right engineer and reply with concrete
              next steps, not a templated brochure.
            </p>
            <div className="mt-8 rounded-2xl border border-border bg-surface p-6 md:p-8">
              <ContactForm />
            </div>
          </Reveal>

          <Reveal delay={0.1}>
            <div className="space-y-4 lg:sticky lg:top-24">
              <Eyebrow className="mb-2">Other ways</Eyebrow>
              <h3 className="text-2xl font-semibold tracking-tight">
                Find us elsewhere.
              </h3>

              <ul className="mt-6 space-y-3">
                {socialLinks.map((s) => (
                  <li key={s.label}>
                    <a
                      href={s.href}
                      target="_blank"
                      rel="noreferrer"
                      className="group flex items-start gap-4 rounded-2xl border border-border bg-surface p-5 transition-colors hover:border-brand/40"
                    >
                      <span className="inline-flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-brand-soft text-brand transition-colors group-hover:bg-brand group-hover:text-brand-foreground">
                        <svg
                          width="18"
                          height="18"
                          viewBox="0 0 24 24"
                          fill={s.fill ? "currentColor" : "none"}
                          stroke={s.fill ? "none" : "currentColor"}
                          strokeWidth="1.6"
                          aria-hidden="true"
                        >
                          {s.icon}
                        </svg>
                      </span>
                      <div className="flex-1">
                        <p className="font-mono text-[10px] uppercase tracking-[0.18em] text-muted">
                          {s.label}
                        </p>
                        <p className="mt-1 text-sm text-foreground">{s.handle}</p>
                      </div>
                      <span
                        aria-hidden="true"
                        className="text-muted transition-all group-hover:translate-x-1 group-hover:text-brand"
                      >
                        →
                      </span>
                    </a>
                  </li>
                ))}
              </ul>

              <div className="rounded-2xl border border-brand/40 bg-brand-soft/30 p-5">
                <p className="font-mono text-[10px] uppercase tracking-[0.18em] text-brand">
                  Office hours
                </p>
                <p className="mt-2 text-sm text-foreground">
                  Mon — Fri · 9 am – 6 pm Pacific
                </p>
                <p className="mt-1 text-sm text-muted-foreground">
                  Outside hours? Email still works; we'll reply first thing the next
                  business day.
                </p>
              </div>
            </div>
          </Reveal>
        </div>
      </Section>
    </>
  );
}
