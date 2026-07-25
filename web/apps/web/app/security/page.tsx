import { Badge, Button, Eyebrow, Reveal, Section } from "@nativelink/ui";

export const metadata = {
  title: "Security",
  description:
    "NativeLink's security program: organizational, cloud, access, and vendor controls, aligned to SOC 2.",
};

const certifications = [
  {
    name: "SOC 2 Type II",
    status: "Complete",
    statusTone: "success" as const,
    body: "Independently audited against the AICPA Trust Services Criteria. Report available under NDA.",
  },
  {
    name: "ISO 27001",
    status: "Complete",
    statusTone: "success" as const,
    body: "Certified against the ISO/IEC 27001 information security management standard.",
  },
  {
    name: "GDPR",
    status: "Compliant",
    statusTone: "success" as const,
    body: "Data Processing Addendum available on request. Self-host for data residency.",
  },
  {
    name: "CCPA / CPRA",
    status: "Compliant",
    statusTone: "success" as const,
    body: "California residents can request access, deletion, and opt-out at any time.",
  },
];

type Control = {
  title: string;
  body: string;
  links?: { label: string; href: string }[];
};

const groups: { eyebrow: string; title: string; controls: Control[] }[] = [
  {
    eyebrow: "Organizational security",
    title: "How we run the program.",
    controls: [
      {
        title: "Information Security Program",
        body: "We have an Information Security Program in place that is communicated throughout the organization. It follows the criteria set forth by the SOC 2 Framework, a widely known information security auditing procedure created by the American Institute of Certified Public Accountants.",
      },
      {
        title: "Third-party audits",
        body: "Our organization undergoes independent third-party assessments to test our security and compliance controls.",
      },
      {
        title: "Third-party penetration testing",
        body: "We perform independent third-party penetration testing at least annually to ensure that the security posture of our services is uncompromised.",
      },
      {
        title: "Roles and responsibilities",
        body: "Roles and responsibilities related to our Information Security Program and the protection of our customers' data are well defined and documented. Team members are required to review and accept all of the security policies.",
      },
      {
        title: "Security awareness training",
        body: "Our team members are required to complete security awareness training covering industry-standard practices and information security topics such as phishing and password management.",
      },
      {
        title: "Confidentiality",
        body: "All team members are required to sign and adhere to an industry-standard confidentiality agreement prior to their first day of work. Because many of our customers operate in security-focused and regulated environments, we take it a step further: confidentiality obligations continue after employment ends, access to customer data is strictly need-to-know, and we are glad to sign customer-specific NDAs on request.",
      },
      {
        title: "Background checks",
        body: "We perform background checks on all new team members in accordance with local laws.",
      },
    ],
  },
  {
    eyebrow: "Cloud security",
    title: "Where your data lives, and how it's protected.",
    controls: [
      {
        title: "Cloud infrastructure security",
        body: "Our services are hosted with Amazon Web Services (AWS), Google Cloud Platform (GCP), and Convex. They each run robust security programs with multiple certifications.",
        links: [
          { label: "AWS Security", href: "https://aws.amazon.com/security/" },
          { label: "GCP Security", href: "https://cloud.google.com/security" },
          { label: "Convex Security", href: "https://www.convex.dev/security" },
        ],
      },
      {
        title: "Data hosting security",
        body: "Our data is hosted on AWS, GCP, and Convex, located in the United States. See the vendor documentation referenced under Cloud infrastructure security for more information.",
      },
      {
        title: "Encryption at rest",
        body: "All databases are encrypted at rest.",
      },
      {
        title: "Encryption in transit",
        body: "Our applications encrypt data in transit with TLS/SSL.",
      },
      {
        title: "Vulnerability scanning",
        body: "We perform vulnerability scanning and actively monitor for threats.",
      },
      {
        title: "Logging and monitoring",
        body: "We log activity across our AWS and GCP cloud infrastructure and monitor it for security and operational issues.",
      },
      {
        title: "Business continuity and disaster recovery",
        body: "We use our data hosting provider's backup services to reduce the risk of data loss in the event of a hardware failure, and monitoring services to alert the team to any failures affecting users.",
      },
      {
        title: "Incident response",
        body: "We have a process for handling information security events that includes escalation procedures, rapid mitigation, and communication.",
      },
    ],
  },
  {
    eyebrow: "Access security",
    title: "Who can reach what.",
    controls: [
      {
        title: "Permissions and authentication",
        body: "Access to cloud infrastructure and other sensitive tools is limited to authorized employees who require it for their role. Where available, we use Single Sign-On (SSO), two-factor authentication (2FA), and strong password policies to protect access to cloud services.",
      },
      {
        title: "Least privilege access control",
        body: "We follow the principle of least privilege with respect to identity and access management.",
      },
      {
        title: "Quarterly access reviews",
        body: "We perform quarterly access reviews of all team members with access to sensitive systems.",
      },
      {
        title: "Password requirements",
        body: "All team members are required to adhere to a minimum set of password requirements and complexity for access.",
      },
      {
        title: "Password managers",
        body: "All company-issued laptops use a password manager for team members to manage passwords and maintain complexity.",
      },
    ],
  },
  {
    eyebrow: "Vendor and risk management",
    title: "Managing third parties and risk.",
    controls: [
      {
        title: "Annual risk assessments",
        body: "We undergo at least annual risk assessments to identify potential threats, including considerations for fraud.",
      },
      {
        title: "Vendor risk management",
        body: "Vendor risk is determined and the appropriate vendor reviews are performed prior to authorizing a new vendor.",
      },
    ],
  },
];

export default function SecurityPage() {
  return (
    <>
      {/* HERO */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[500px] bg-[radial-gradient(ellipse_900px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.13),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-16 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[860px] text-center">
              <Eyebrow className="mb-5">Security</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] md:text-[64px]">
                Security you can{" "}
                <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
                  audit, not just trust
                </span>
                .
              </h1>
              <p className="mx-auto mt-6 max-w-[640px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                Our security program is aligned to the SOC 2 framework and covers
                our organization, our cloud infrastructure, access, and vendors.
                The controls below describe how we protect your data.
              </p>
              <div className="mt-8 flex flex-wrap justify-center gap-3">
                <Button size="lg" asChild>
                  <a href="mailto:security@tracemachina.com">Contact security</a>
                </Button>
                <Button size="lg" variant="outline" asChild>
                  <a href="mailto:security@tracemachina.com?subject=Report%20request">Request a report</a>
                </Button>
              </div>
            </div>
          </Reveal>
        </Section>
      </section>

      {/* CERTIFICATIONS */}
      <Section width="default" className="border-y border-border/60 bg-surface-elevated/40 py-24">
        <Reveal>
          <div className="mb-12 max-w-[680px]">
            <Eyebrow className="mb-4">Certifications</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Independently verified.
            </h2>
          </div>
        </Reveal>
        <div className="grid auto-rows-fr gap-5 md:grid-cols-2">
          {certifications.map((c, i) => (
            <Reveal key={c.name} delay={i * 0.05}>
              <div className="flex h-full flex-col rounded-2xl border border-border bg-surface p-7">
                <div className="flex items-start justify-between gap-4">
                  <h3 className="text-xl font-semibold tracking-tight text-foreground">
                    {c.name}
                  </h3>
                  <Badge variant={c.statusTone}>{c.status}</Badge>
                </div>
                <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                  {c.body}
                </p>
              </div>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* CONTROL GROUPS */}
      {groups.map((group) => (
        <Section key={group.eyebrow} width="default" className="border-b border-border/60 py-24">
          <Reveal>
            <div className="mb-12 max-w-[680px]">
              <Eyebrow className="mb-4">{group.eyebrow}</Eyebrow>
              <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
                {group.title}
              </h2>
            </div>
          </Reveal>
          <div className="flex flex-wrap justify-center gap-5">
            {group.controls.map((c, i) => (
              <Reveal
                key={c.title}
                delay={i * 0.04}
                className="w-full sm:w-[calc(50%-0.625rem)] lg:w-[calc(33.333%-0.834rem)]"
              >
                <div className="h-full rounded-2xl border border-border bg-surface p-6">
                  <h3 className="text-lg font-semibold leading-tight tracking-tight text-foreground">
                    {c.title}
                  </h3>
                  <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                    {c.body}
                  </p>
                  {c.links && (
                    <div className="mt-4 flex flex-wrap gap-x-4 gap-y-1">
                      {c.links.map((l) => (
                        <a
                          key={l.href}
                          href={l.href}
                          target="_blank"
                          rel="noreferrer"
                          className="text-sm font-medium text-brand underline-offset-4 hover:underline"
                        >
                          {l.label}
                        </a>
                      ))}
                    </div>
                  )}
                </div>
              </Reveal>
            ))}
          </div>
        </Section>
      ))}

      {/* CONTACT */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-0 -z-10 bg-[radial-gradient(ellipse_at_center,rgb(var(--nl-color-brand)/0.1),transparent_60%)]" />
        <Section width="narrow" className="py-24 text-center">
          <Reveal>
            <Eyebrow className="mb-5">Contact us</Eyebrow>
            <h2 className="text-balance text-3xl font-semibold leading-[1.1] tracking-[-0.025em] md:text-4xl">
              Questions, or something to report?
            </h2>
            <p className="mx-auto mt-5 max-w-[560px] text-base leading-relaxed text-muted-foreground md:text-lg">
              If you have any questions, comments, or concerns, or wish to report
              a potential security issue, please contact us.
            </p>
            <div className="mt-8 flex justify-center">
              <Button size="lg" asChild>
                <a href="mailto:security@tracemachina.com">security@tracemachina.com</a>
              </Button>
            </div>
          </Reveal>
        </Section>
      </section>
    </>
  );
}
