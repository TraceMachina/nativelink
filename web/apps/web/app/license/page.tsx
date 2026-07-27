import { Eyebrow, Reveal, Section } from "@nativelink/ui";

export const metadata = {
  title: "License",
  description:
    "How NativeLink's source-available, FSL-Apache, and Business Source License modules fit together.",
};

const toc = [
  { id: "overview", label: "Overview" },
  { id: "individual-cache", label: "Individual cache use" },
  { id: "commercial-modules", label: "Commercial modules" },
  { id: "contributors", label: "Contributor waivers" },
  { id: "source", label: "Source of truth" },
];

export default function LicensePage() {
  return (
    <>
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[400px] bg-[radial-gradient(ellipse_800px_400px_at_50%_-20%,rgb(var(--nl-color-brand)/0.10),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-12 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[820px] text-center">
              <Eyebrow className="mb-5">License</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.05] tracking-[-0.035em] md:text-[60px]">
                NativeLink Licensing
              </h1>
              <p className="mx-auto mt-5 max-w-[650px] text-base leading-relaxed text-muted-foreground md:text-lg">
                NativeLink is a monorepo with module-aware licensing. Most code is
                FSL-Apache; selected commercial modules use the Business Source
                License.
              </p>
            </div>
          </Reveal>
        </Section>
      </section>

      <Section width="default" className="pb-24">
        <div className="grid gap-12 lg:grid-cols-[220px_1fr]">
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

          <article className="prose-styles max-w-[68ch]">
            <Reveal>
              <h2 id="overview" className="scroll-mt-24 text-3xl font-semibold tracking-tight text-foreground">
                Overview
              </h2>
              <p className="mt-4 text-[15px] leading-relaxed text-muted-foreground">
                Most NativeLink modules are licensed under
                FSL-1.1-Apache-2.0. That license allows internal use,
                modification, and redistribution for non-competing purposes, then
                grants an Apache 2.0 future license on the schedule described in
                the repository license.
              </p>
              <p className="mt-4 text-[15px] leading-relaxed text-muted-foreground">
                NativeLink is not a single-license repository. Some files and
                modules carry their own source headers, and those headers control
                the license for that code.
              </p>

              <h3 id="individual-cache" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                Individual Cache Use
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                Developers using NativeLink for an individual cache do not need a
                commercial license. That path is meant to stay simple: run the
                cache locally or on infrastructure you control, point your build
                tool at it, and keep your loop fast.
              </p>

              <h3 id="commercial-modules" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                Commercial Modules
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                Metrics and remote persistent workers are licensed under the
                Business Source License. Teams using those modules in shared,
                production, or commercial settings should use NativeLink Cloud,
                Enterprise, or a separate commercial license.
              </p>
              <p className="mt-4 text-[15px] leading-relaxed text-muted-foreground">
                The separate commercial license is intentionally very inexpensive.
                It is there to keep the project sustainable without putting
                friction in front of individual developers.
              </p>

              <h3 id="contributors" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                Contributor Waivers
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                Meaningful contributors may be eligible for license waivers for
                Business Source License modules. Open an issue or contact the
                maintainers before depending on a waiver.
              </p>

              <h3 id="source" className="mt-10 scroll-mt-24 text-xl font-semibold tracking-tight text-foreground">
                Source of Truth
              </h3>
              <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                The repository
                {" "}
                <a href="https://github.com/TraceMachina/nativelink/blob/main/LICENSE">LICENSE</a>
                {" "}
                file and source headers are the authoritative licensing text. For
                commercial terms, contact
                {" "}
                <a href="mailto:contact@tracemachina.com">contact@tracemachina.com</a>.
              </p>
            </Reveal>
          </article>
        </div>
      </Section>
    </>
  );
}
