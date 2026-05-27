import { Eyebrow, Reveal, Section } from "@nativelink/ui";

export const metadata = {
  title: "Terms & Privacy",
  description: "Terms of Service and Privacy Policy for NativeLink.",
};

const toc = [
  { id: "terms", label: "Terms of Service" },
  { id: "acceptance", label: "Acceptance" },
  { id: "license", label: "License & use" },
  { id: "accounts", label: "Accounts" },
  { id: "acceptable-use", label: "Acceptable use" },
  { id: "privacy", label: "Privacy Policy" },
  { id: "data", label: "What we collect" },
  { id: "cookies", label: "Cookies" },
  { id: "rights", label: "Your rights" },
  { id: "contact", label: "Contact" },
];

export default function TermsPage() {
  return (
    <>
      {/* HERO */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[400px] bg-[radial-gradient(ellipse_800px_400px_at_50%_-20%,rgb(var(--nl-color-brand)/0.10),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-12 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[820px] text-center">
              <Eyebrow className="mb-5">Legal</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.05] tracking-[-0.035em] md:text-[60px]">
                Terms & Privacy
              </h1>
              <p className="mx-auto mt-5 max-w-[600px] text-base leading-relaxed text-muted-foreground md:text-lg">
                The terms that govern your use of NativeLink and our commitment to
                handling your data responsibly.
              </p>
              <p className="mt-4 font-mono text-xs uppercase tracking-[0.18em] text-muted">
                Last updated · April 12, 2026
              </p>
            </div>
          </Reveal>
        </Section>
      </section>

      <Section width="default" className="pb-24">
        <div className="grid gap-12 lg:grid-cols-[220px_1fr]">
          {/* TOC */}
          <aside className="hidden lg:block">
            <div className="sticky top-24">
              <p className="mb-4 font-mono text-[10px] uppercase tracking-[0.18em] text-muted">
                On this page
              </p>
              <ul className="space-y-2 text-sm">
                {toc.map((item) => (
                  <li key={item.id}>
                    <a
                      href={`#${item.id}`}
                      className="block text-muted-foreground transition-colors hover:text-brand"
                    >
                      {item.label}
                    </a>
                  </li>
                ))}
              </ul>
            </div>
          </aside>

          {/* CONTENT */}
          <article className="prose-styles max-w-[68ch]">
            <Reveal>
              <h2 id="terms" className="scroll-mt-24 text-3xl font-semibold tracking-tight text-foreground">
                Terms of Service
              </h2>
              <p className="mt-4 text-[15px] leading-relaxed text-muted-foreground">
                These Terms of Service govern your use of NativeLink, the hosted
                cloud service, and any related products operated by Trace Machina
                ("we," "us"). By accessing or using the service, you agree to be
                bound by these terms.
              </p>

              <h3 id="acceptance" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                1. Acceptance of Terms
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                By creating an account, deploying NativeLink, or otherwise using our
                service, you confirm that you have read these terms, agree to them,
                and have the authority to bind your organization where applicable.
              </p>

              <h3 id="license" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                2. License & Use
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                The NativeLink open-source core is distributed under the MIT
                License. You are free to use, modify, and distribute the open-source
                release subject to that license. NativeLink Cloud and Enterprise
                offerings are provided under the separate agreement you accept when
                you sign up for those services.
              </p>

              <h3 id="accounts" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                3. Accounts
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                You are responsible for safeguarding any credentials issued to your
                account and for all activity that occurs under those credentials.
                Notify us promptly of any unauthorized use.
              </p>

              <h3 id="acceptable-use" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                4. Acceptable Use
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                You agree not to use the service to violate any law, infringe
                third-party rights, distribute malware, or interfere with the
                integrity or performance of the service or its users.
              </p>

              <div className="my-12 h-px w-full bg-border" />

              <h2 id="privacy" className="scroll-mt-24 text-3xl font-semibold tracking-tight text-foreground">
                Privacy Policy
              </h2>
              <p className="mt-4 text-[15px] leading-relaxed text-muted-foreground">
                We treat your data as if it were our own — because if you're running
                builds on our infrastructure, in a real sense it is. This policy
                describes what we collect, why, and your rights over that data.
              </p>

              <h3 id="data" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                What we collect
              </h3>
              <ul className="mt-3 space-y-2 pl-6 text-[15px] leading-relaxed text-muted-foreground [&_li]:list-disc">
                <li>
                  Account information (name, email, organization) when you sign up
                  for Cloud or Enterprise.
                </li>
                <li>
                  Build metadata (action digests, timing, success/failure) for the
                  purpose of running and improving the service. Build artifacts
                  themselves are content-addressed and never inspected by us.
                </li>
                <li>
                  Anonymous usage telemetry from the open-source release, which can
                  be disabled with a single flag.
                </li>
                <li>
                  Standard server logs (IP addresses, user agents) retained for 30
                  days for security and abuse detection.
                </li>
              </ul>

              <h3 id="cookies" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                Cookies
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                We use first-party cookies to keep you signed in and to remember
                your preferences. Analytics is run server-side with no third-party
                tracking cookies set on this site.
              </p>

              <h3 id="rights" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                Your rights
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                You may request access to, correction of, or deletion of personal
                data we hold about you at any time by emailing{" "}
                <a href="mailto:privacy@nativelink.com" className="text-brand underline-offset-4 hover:underline">
                  privacy@nativelink.com
                </a>
                . If you're an EU/UK resident, this includes the rights granted
                under GDPR / UK GDPR.
              </p>

              <h3 id="contact" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                Contact
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                Questions about these terms or our privacy practices? Email{" "}
                <a href="mailto:legal@nativelink.com" className="text-brand underline-offset-4 hover:underline">
                  legal@nativelink.com
                </a>{" "}
                or write to Trace Machina, Attn: Legal, 548 Market St, San
                Francisco, CA 94104.
              </p>
            </Reveal>
          </article>
        </div>
      </Section>
    </>
  );
}
