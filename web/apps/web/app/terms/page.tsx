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
  { id: "consent", label: "Consent & withdrawal" },
  { id: "contact", label: "Contact" },
  { id: "changelog", label: "Change history" },
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
                Last updated · July 24, 2026
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
                By agreeing, you also consent to our collection and processing of
                personal data as described in the{" "}
                <a href="#privacy" className="text-brand underline-offset-4 hover:underline">
                  Privacy Policy
                </a>{" "}
                below. Your consent is specific and informed, and you may withdraw
                it at any time (see{" "}
                <a href="#consent" className="text-brand underline-offset-4 hover:underline">
                  Consent &amp; withdrawal
                </a>
                ).
              </p>

              <h3 id="license" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                2. License & Use
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                NativeLink is distributed from a monorepo with module-aware
                licensing. Most modules are licensed under FSL-1.1-Apache-2.0,
                while some commercial modules, including metrics and remote
                persistent workers, are licensed under the Business Source License.
                Developers using NativeLink for an individual cache do not need a
                commercial license; team and production usage of commercial modules
                can be covered by an intentionally very inexpensive separate license.
                See the <a href="/license">license page</a> for details.
                NativeLink Cloud and Enterprise offerings are provided under the
                separate agreement you accept when you sign up for those services.
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
                  Anonymous usage telemetry from the source-available release,
                  which can be disabled with a single flag.
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
                <a href="mailto:privacy@tracemachina.com" className="text-brand underline-offset-4 hover:underline">
                  privacy@tracemachina.com
                </a>
                . If you're an EU/UK resident, this includes the rights granted
                under GDPR / UK GDPR.
              </p>

              <h3 id="consent" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                Consent &amp; withdrawal
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                Where we rely on your consent to process personal data, that
                consent is freely given, specific, and informed. You give it when
                you agree to these terms during sign-up, and the processing it
                covers is the collection and use of the data described in{" "}
                <a href="#data" className="text-brand underline-offset-4 hover:underline">
                  What we collect
                </a>{" "}
                for the purposes of providing, securing, and improving the service.
              </p>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                You may withdraw your consent at any time, without affecting the
                lawfulness of processing carried out before withdrawal. To
                withdraw, request the Data Subject Consent Withdrawal Form by
                emailing{" "}
                <a href="mailto:privacy@tracemachina.com?subject=Data%20Subject%20Consent%20Withdrawal" className="text-brand underline-offset-4 hover:underline">
                  privacy@tracemachina.com
                </a>{" "}
                or write to us at the postal address in{" "}
                <a href="#contact" className="text-brand underline-offset-4 hover:underline">
                  Contact
                </a>
                . We keep a record of the consents we rely on and honor withdrawal
                requests promptly.
              </p>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                We do not sell your personal information, and we do not share it
                for cross-context behavioral advertising. California residents may
                exercise their rights under the CPRA using the same contact
                methods above.
              </p>

              <h3 id="contact" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                Contact
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                Questions about these terms or our privacy practices? Email{" "}
                <a href="mailto:legal@tracemachina.com" className="text-brand underline-offset-4 hover:underline">
                  legal@tracemachina.com
                </a>{" "}
                or write to Trace Machina, Inc., Attn: Legal, PO Box 60676, 265
                Cambridge Ave, Palo Alto, CA 94306.
              </p>

              <div className="my-12 h-px w-full bg-border" />

              <h2 id="changelog" className="scroll-mt-24 text-3xl font-semibold tracking-tight text-foreground">
                Change history
              </h2>
              <p className="mt-4 text-[15px] leading-relaxed text-muted-foreground">
                Material changes to the Terms of Service and Privacy Policy are
                recorded here.
              </p>
              <ul className="mt-3 space-y-2 pl-6 text-[15px] leading-relaxed text-muted-foreground [&_li]:list-disc">
                <li>
                  <span className="font-mono text-[13px] text-foreground">July 24, 2026</span>:
                  Added a data-processing consent clause and a Consent &amp;
                  withdrawal section covering how consent is given, how it can be
                  withdrawn, and that we do not sell or share personal data.
                </li>
                <li>
                  <span className="font-mono text-[13px] text-foreground">July 24, 2026</span> —
                  Added this change history section. No changes to the terms or
                  the policy.
                </li>
                <li>
                  <span className="font-mono text-[13px] text-foreground">July 7, 2026</span> —
                  Updated the legal contact address to Trace Machina&apos;s Palo
                  Alto mailing address.
                </li>
                <li>
                  <span className="font-mono text-[13px] text-foreground">May 29, 2026</span> —
                  Updated the license terms to module-aware licensing
                  (FSL-1.1-Apache-2.0, with some commercial modules under the
                  Business Source License) and clarified telemetry wording for
                  the source-available release.
                </li>
                <li>
                  <span className="font-mono text-[13px] text-foreground">April 12, 2026</span> —
                  Prior revision of the Terms of Service and Privacy Policy.
                </li>
              </ul>
            </Reveal>
          </article>
        </div>
      </Section>
    </>
  );
}
