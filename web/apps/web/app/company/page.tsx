import {
  Button,
  Card,
  CardBody,
  CardFooter,
  CardHeader,
  CardTitle,
  Eyebrow,
  Reveal,
  Section,
} from "@nativelink/ui";

export const metadata = { title: "Company" };

const values = [
  {
    title: "Engineer like it matters.",
    body: "We work on infrastructure that other engineers depend on. The bar is correctness, then performance, then everything else.",
  },
  {
    title: "Default to open.",
    body: "Most of the monorepo is FSL-1.1-Apache-2.0. We keep the code visible, and we sell support, operations, and commercial terms for selected modules.",
  },
  {
    title: "Speed is a feature.",
    body: "Slow builds cost developer time, focus, and willpower. We measure ourselves in milliseconds, not feature checkboxes.",
  },
  {
    title: "Hard problems, kindly.",
    body: "Distributed systems are unforgiving; humans don't have to be. We ship rigorous code with patient code review.",
  },
];

const contactCards = [
  {
    title: "Media kit",
    body: "Logos, branding guidelines, and approved press resources.",
    cta: { label: "Download", href: "https://drive.google.com/drive/folders/" },
    variant: "outline" as const,
  },
  {
    title: "General inquiries",
    body: "Have a question or comment? We'd love to hear from you.",
    cta: { label: "Contact us", href: "mailto:contact@nativelink.com" },
    variant: "outline" as const,
  },
  {
    title: "Sales inquiries",
    body: "Curious if NativeLink fits your team? Our sales engineers can help.",
    cta: { label: "Talk to sales", href: "mailto:hello@nativelink.com" },
    variant: "primary" as const,
  },
];

export default function CompanyPage() {
  return (
    <>
      {/* HERO */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[600px] bg-[radial-gradient(ellipse_1000px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.16),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-16 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[920px] text-center">
              <Eyebrow className="mb-5">Company</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] md:text-[68px]">
                Accelerating the{" "}
                <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
                  machine age
                </span>
                .
              </h1>
              <p className="mx-auto mt-6 max-w-[680px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                NativeLink is built by Trace Machina. Our mission is to accelerate the
                reindustrialization of the world by making advanced build and simulation processes
                that move at machine speed.
              </p>
            </div>
          </Reveal>
        </Section>
      </section>

      {/* MISSION */}
      <Section width="narrow" className="pb-24">
        <Reveal>
          <div className="space-y-6 text-balance text-center text-base leading-relaxed text-muted-foreground md:text-lg">
            <p className="text-2xl font-semibold leading-[1.3] tracking-[-0.02em] text-foreground md:text-3xl">
              Mission: Accelerate the progress of humanity in the machine age.
            </p>
            <p>
              Our products amplify the rate at which companies can innovate across mission-critical
              industries — from semiconductors and advanced robotics to autonomous vehicles, AI
              research, life sciences, and financial services.
            </p>
            <p>
              Our commitment is to amplify execution across any environment, ensuring that
              developers can focus on creating transformative technologies that drive forward
              progress on behalf of humanity.
            </p>
          </div>
        </Reveal>
      </Section>

      {/* VALUES */}
      <Section width="default" className="border-y border-border/60 bg-surface-elevated/40 py-24">
        <Reveal>
          <div className="mb-14 max-w-[600px]">
            <Eyebrow className="mb-4">Values</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              How we work.
            </h2>
          </div>
        </Reveal>
        <div className="grid gap-5 md:grid-cols-2">
          {values.map((v, i) => (
            <Reveal key={v.title} delay={i * 0.06}>
              <div className="group h-full rounded-2xl border border-border bg-surface p-8 transition-colors hover:border-brand/40">
                <div className="mb-6 inline-flex h-10 w-10 items-center justify-center rounded-xl bg-brand-soft font-mono text-sm font-semibold text-brand">
                  0{i + 1}
                </div>
                <h3 className="text-2xl font-semibold leading-tight tracking-tight text-foreground">
                  {v.title}
                </h3>
                <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">{v.body}</p>
              </div>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* CONTACT */}
      <Section width="default" className="border-t border-border/60 bg-surface-elevated/40 py-24">
        <Reveal>
          <div className="mx-auto mb-12 max-w-[680px] text-center">
            <Eyebrow className="mb-4">Get in touch</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Have questions or need assistance?
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted-foreground md:text-lg">
              Reach out for general inquiries, sales conversations, or anything else — we usually
              reply within a day.
            </p>
          </div>
        </Reveal>
        <div className="grid gap-5 md:grid-cols-3">
          {contactCards.map((c, i) => (
            <Reveal key={c.title} delay={i * 0.05}>
              <Card
                variant={c.variant === "primary" ? "featured" : "default"}
                className="flex h-full flex-col"
              >
                <CardHeader>
                  <CardTitle>{c.title}</CardTitle>
                </CardHeader>
                <CardBody>{c.body}</CardBody>
                <CardFooter className="mt-auto">
                  <Button asChild variant={c.variant} size="md" className="w-full">
                    <a
                      href={c.cta.href}
                      target={c.cta.href.startsWith("http") ? "_blank" : undefined}
                      rel={c.cta.href.startsWith("http") ? "noreferrer" : undefined}
                    >
                      {c.cta.label}
                    </a>
                  </Button>
                </CardFooter>
              </Card>
            </Reveal>
          ))}
        </div>
      </Section>
    </>
  );
}
