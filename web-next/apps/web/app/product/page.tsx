import {
  Badge,
  Button,
  Card,
  CardBody,
  CardHeader,
  CardTitle,
  Eyebrow,
  FAQ,
  type FAQItem,
  Reveal,
  Section,
} from "@nativelink/ui";

export const metadata = { title: "Product" };

const pillars = [
  {
    eyebrow: "Remote cache",
    title: "Cache once. Reuse forever.",
    body: "Content-addressable storage deduplicates every artifact your team produces. If a teammate, your CI, or an agent has already built it, you get it back in milliseconds. Drops into Bazel, Buck2, Reclient, Pants, Goma — and CMake via recc.",
    proof: "Over a billion build requests served per month, in production.",
  },
  {
    eyebrow: "Remote build execution",
    title: "Distribute across every core you have.",
    body: "Offload compilation and tests to a worker fleet that scales horizontally — on AWS, GCP, or bare metal. Hermetic by design, deterministic by default. Specialized hardware (GPUs, ARM, Apple Silicon) supported natively.",
  },
  {
    eyebrow: "Cloud & self-host",
    title: "Hosted by us. Or run by you.",
    body: "Start free on NativeLink Cloud in ten minutes. Move to dedicated infrastructure when your scale demands it. Or self-host the open-source release with one Docker command. Same code path. Same performance.",
  },
  {
    eyebrow: "Built on Rust",
    title: "Performance that doesn't stall.",
    body: "No garbage collector. No race conditions at scale. No mystery latency spikes. Memory safety without the runtime tax — which is why NativeLink can serve a billion requests a month on infrastructure that would buckle other systems.",
  },
];

const integrationGroups = [
  {
    label: "AI coding platforms",
    items: "Claude Code, GitHub Copilot Workspace, Devin, Cursor, Windsurf, and more.",
  },
  { label: "Languages", items: "C++, Rust, Python, Go, Java, Kotlin, Swift, and more." },
  {
    label: "Build systems",
    items: "Bazel, Buck2, Reclient, Soong, Pants, Goma — and CMake via recc.",
  },
  { label: "Cloud", items: "AWS, GCP, and Azure." },
  { label: "CI", items: "GitHub Actions, GitLab CI, Buildkite, Jenkins." },
];

const faqItems: FAQItem[] = [
  {
    q: "What is NativeLink?",
    a: "NativeLink is a high-performance build cache and remote execution system designed to accelerate software compilation and testing while reducing infrastructure costs.",
  },
  {
    q: "How do I set up NativeLink?",
    a: "Deploy it as a Docker image. Detailed setup instructions live in the NativeLink documentation.",
  },
  {
    q: "What operating systems are supported?",
    a: "NativeLink supports Unix-based operating systems and Windows, ensuring broad compatibility across development environments.",
  },
  {
    q: "What are the benefits?",
    a: "Significantly reduced build times — especially for incremental changes — by storing and reusing previous build results. It also distributes build and test tasks across multiple machines.",
  },
  {
    q: "Is NativeLink free?",
    a: "The open-source version is free to set up. It's fully open source — you self-host and operate it.",
  },
  {
    q: "Why Rust?",
    a: "NativeLink's Rust-based architecture ensures hermeticity and eliminates garbage collection pauses and race conditions — ideal for complex and safety-critical codebases.",
  },
  {
    q: "How does remote execution work?",
    a: "NativeLink distributes build and test tasks across a network of machines, parallelizing workloads and offloading computational burdens from local machines.",
  },
  {
    q: "Does it work with my existing tools?",
    a: "Yes — NativeLink integrates with any build tool that speaks the Remote Execution protocol, including Bazel, Buck2, Goma, and Reclient.",
  },
  {
    q: "How do I keep my Bazel setup hermetic?",
    a: "NativeLink eliminates external dependencies and maintains consistency across builds. The documentation covers configuration in detail.",
  },
  {
    q: "Why should I choose NativeLink?",
    a: "Trusted by large engineering organizations for cutting build costs and developer iteration time. Handles over one billion requests per month and is designed for scale and reliability.",
  },
];

export default function ProductPage() {
  return (
    <>
      {/* HERO */}
      <Section width="default" className="pt-24 pb-16 md:pt-32">
        <Reveal>
          <div className="mx-auto max-w-[820px] text-center">
            <Eyebrow className="mb-5">Product</Eyebrow>
            <h1 className="text-balance text-4xl font-bold leading-[1.1] tracking-tight md:text-6xl">
              One platform. Every build. Every machine.
            </h1>
            <p className="mx-auto mt-6 max-w-[640px] text-lg leading-relaxed text-muted">
              NativeLink unifies remote caching, remote execution, and observability
              into a single Rust-native platform — built to keep up with codebases that
              grow faster than you can provision them.
            </p>
            <div className="mt-8 flex flex-wrap justify-center gap-4">
              <Button size="lg" asChild>
                <a href="/docs">Start in 10 minutes</a>
              </Button>
              <Button size="lg" variant="outline" asChild>
                <a href="mailto:contact@nativelink.com">Talk to us</a>
              </Button>
            </div>
          </div>
        </Reveal>
      </Section>

      {/* PILLARS */}
      <Section
        width="default"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-20"
      >
        <div className="grid gap-6 md:grid-cols-2">
          {pillars.map((p, i) => (
            <Reveal key={p.title} delay={i * 0.05}>
              <Card className="h-full">
                <CardHeader>
                  <Eyebrow className="mb-2">{p.eyebrow}</Eyebrow>
                  <CardTitle className="text-2xl">{p.title}</CardTitle>
                </CardHeader>
                <CardBody>{p.body}</CardBody>
                {p.proof ? (
                  <p className="mt-6 border-t border-border pt-4 font-mono text-xs uppercase tracking-widest text-muted">
                    {p.proof}
                  </p>
                ) : null}
              </Card>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* SECURITY & PROVENANCE */}
      <Section
        width="default"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-24"
      >
        <div className="grid gap-12 lg:grid-cols-[1fr_1.4fr] lg:gap-20">
          <Reveal>
            <Eyebrow className="mb-4">Security & provenance</Eyebrow>
            <h2 className="text-3xl font-bold leading-[1.15] md:text-4xl">
              Trust at every hop. Provenance at every step.
            </h2>
          </Reveal>
          <Reveal delay={0.1}>
            <div className="space-y-6 text-base leading-relaxed text-muted md:text-lg">
              <p>
                SSO/SAML, signed worker inputs and outputs, packet integrity, end-to-end
                TLS. Build artifacts are content-addressed and tamper-evident by design.
                Hermetic execution means no surprise dependencies pulled in mid-build —
                every input explicit, every output verifiable.
              </p>
              <p>
                Because builds are programmable, your security and observability tools
                plug straight into the data: dependency graphs, execution metadata,
                artifact provenance — queryable, exportable, auditable. Critical when
                humans are committing code. Existential when agents are.
              </p>
            </div>
          </Reveal>
        </div>
      </Section>

      {/* INTEGRATIONS */}
      <Section
        width="default"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-24"
      >
        <Reveal>
          <div className="mx-auto mb-12 max-w-[680px] text-center">
            <Eyebrow className="mb-4">Integrations</Eyebrow>
            <h2 className="text-3xl font-bold leading-[1.15] md:text-5xl">
              Works with your stack.
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted md:text-lg">
              NativeLink speaks the standard remote-execution APIs — if it builds your
              code today, it'll build it on NativeLink tomorrow.
            </p>
          </div>
        </Reveal>
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {integrationGroups.map((g, i) => (
            <Reveal key={g.label} delay={i * 0.04}>
              <div className="h-full rounded-md border-2 border-border p-6">
                <Eyebrow className="mb-3">{g.label}</Eyebrow>
                <p className="text-base leading-relaxed text-foreground">{g.items}</p>
              </div>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* LLVM CASE STUDY */}
      <Section
        width="narrow"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-24 text-center"
      >
        <Reveal>
          <Eyebrow className="mb-5">Proof at scale</Eyebrow>
          <h2 className="text-balance text-3xl font-bold leading-[1.15] md:text-5xl">
            LLVM builds 4× faster on NativeLink.
          </h2>
          <p className="mx-auto mt-6 max-w-[620px] text-base leading-relaxed text-muted md:text-lg">
            LLVM contributors are using NativeLink with CMake and recc to distribute
            builds of clang and the LLVM toolchain — cutting full-project compile time
            from 17 minutes to 4. No build-system migration. No proprietary client.
            Just your existing CMake setup, pointed at NativeLink.
          </p>
          <div className="mt-8 flex justify-center">
            <Button variant="link" asChild>
              <a
                href="https://reidkleckner.dev/posts/llvm-recc-nativelink/"
                target="_blank"
                rel="noreferrer"
              >
                Read the write-up →
              </a>
            </Button>
          </div>
          <div className="mt-8 flex flex-wrap justify-center gap-3">
            <Badge variant="outline">17 min → 4 min</Badge>
            <Badge variant="outline">CMake + recc</Badge>
            <Badge variant="outline">No build-system migration</Badge>
          </div>
        </Reveal>
      </Section>

      {/* FAQ */}
      <Section
        width="default"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-24"
      >
        <Reveal>
          <div className="mx-auto mb-12 max-w-[680px] text-center">
            <Eyebrow className="mb-4">FAQ</Eyebrow>
            <h2 className="text-3xl font-bold leading-[1.15] md:text-5xl">
              Frequently asked questions
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted md:text-lg">
              Answers to the questions teams ask most often, from implementation
              details to support options.
            </p>
          </div>
        </Reveal>
        <Reveal>
          <div className="mx-auto max-w-[820px]">
            <FAQ items={faqItems} />
            <p className="mt-8 text-center text-sm text-muted">
              For more details, see the{" "}
              <a href="/docs" className="underline underline-offset-4">
                NativeLink documentation
              </a>
              .
            </p>
          </div>
        </Reveal>
      </Section>

      {/* BOTTOM CTA */}
      <Section
        width="narrow"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-24 text-center"
      >
        <Reveal>
          <h2 className="text-balance text-3xl font-bold leading-[1.15] md:text-5xl">
            Try it in 10 minutes.
          </h2>
          <div className="mt-8 flex flex-wrap justify-center gap-3">
            <Button size="lg" asChild>
              <a
                href="https://github.com/tracemachina/nativelink"
                target="_blank"
                rel="noreferrer"
              >
                Clone the repo
              </a>
            </Button>
            <Button size="lg" variant="outline" asChild>
              <a href="https://dev.nativelink.com" target="_blank" rel="noreferrer">
                Start free
              </a>
            </Button>
            <Button size="lg" variant="ghost" asChild>
              <a href="mailto:contact@nativelink.com">Talk to us</a>
            </Button>
          </div>
        </Reveal>
      </Section>
    </>
  );
}
