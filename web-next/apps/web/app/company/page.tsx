import {
  Button,
  Card,
  CardBody,
  CardFooter,
  CardHeader,
  CardTitle,
  Divider,
  Eyebrow,
  Reveal,
  Section,
} from "@nativelink/ui";

export const metadata = { title: "Company" };

const contactCards = [
  {
    title: "Media kit",
    body: "Download our logos, branding guidelines, and other press resources.",
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
    body: "Want to know if NativeLink is right for your team? Our sales engineers can help.",
    cta: { label: "Talk to sales", href: "mailto:hello@nativelink.com" },
    variant: "primary" as const,
  },
];

export default function CompanyPage() {
  return (
    <>
      {/* HERO */}
      <Section width="default" className="pt-24 pb-16 md:pt-32">
        <Reveal>
          <div className="mx-auto max-w-[820px] text-center">
            <Eyebrow className="mb-5">Company</Eyebrow>
            <h1 className="text-balance text-4xl font-bold leading-[1.1] tracking-tight md:text-6xl">
              NativeLink is built by Trace Machina.
            </h1>
            <p className="mx-auto mt-6 max-w-[640px] text-lg leading-relaxed text-muted">
              Our mission is to accelerate the reindustrialization of the world in the
              machine age.
            </p>
          </div>
        </Reveal>
      </Section>

      {/* MISSION */}
      <Section width="narrow" className="pb-24">
        <Reveal>
          <div className="space-y-6 text-balance text-center text-base leading-relaxed text-muted md:text-lg">
            <p>
              We empower engineers to build the future of technology by making advanced
              build and simulation processes that move at machine speed.
            </p>
            <p>
              Our products amplify the rate at which companies can innovate across
              mission-critical industries — from semiconductors and advanced robotics to
              autonomous vehicles, artificial intelligence research, life sciences, and
              financial services.
            </p>
            <p>
              Our commitment is to amplify execution across any environment, ensuring
              that developers can focus on creating transformative technologies that
              drive forward progress on behalf of humanity.
            </p>
          </div>
        </Reveal>
      </Section>

      <Section width="default" className="py-0">
        <Divider inset />
      </Section>

      {/* CONTACT CARDS */}
      <Section width="default" className="py-24">
        <Reveal>
          <div className="mx-auto mb-12 max-w-[680px] text-center">
            <Eyebrow className="mb-4">Get in touch</Eyebrow>
            <h2 className="text-3xl font-bold leading-[1.15] md:text-5xl">
              Have questions or need assistance?
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted md:text-lg">
              Reach out for general inquiries, sales conversations, or anything else —
              we usually reply within a day.
            </p>
          </div>
        </Reveal>
        <div className="grid gap-6 md:grid-cols-3">
          {contactCards.map((c, i) => (
            <Reveal key={c.title} delay={i * 0.05}>
              <Card className="flex h-full flex-col">
                <CardHeader>
                  <CardTitle>{c.title}</CardTitle>
                </CardHeader>
                <CardBody>{c.body}</CardBody>
                <CardFooter className="mt-auto">
                  <Button
                    asChild
                    variant={c.variant}
                    size="md"
                    className={c.variant === "outline" ? "w-full" : "w-full"}
                  >
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
