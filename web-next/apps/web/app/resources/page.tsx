import {
  Badge,
  Button,
  Card,
  CardBody,
  CardHeader,
  CardTitle,
  Eyebrow,
  Reveal,
  Section,
  YouTubeEmbed,
} from "@nativelink/ui";

export const metadata = { title: "Resources" };

export default function ResourcesPage() {
  return (
    <>
      {/* HERO */}
      <Section width="default" className="pt-24 pb-16 md:pt-32">
        <Reveal>
          <div className="mx-auto max-w-[820px] text-center">
            <Eyebrow className="mb-5">Resources</Eyebrow>
            <h1 className="text-balance text-4xl font-bold leading-[1.1] tracking-tight md:text-6xl">
              Deep-tech writing for engineers who build the builds.
            </h1>
            <p className="mx-auto mt-6 max-w-[640px] text-lg leading-relaxed text-muted">
              Case studies, conference talks, and write-ups from the team building
              NativeLink — plus highlights from the broader community.
            </p>
          </div>
        </Reveal>
      </Section>

      {/* BOOKS */}
      <Section
        width="default"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-20"
      >
        <Reveal>
          <Eyebrow className="mb-4">Books</Eyebrow>
          <h2 className="text-3xl font-bold leading-[1.15] md:text-4xl">
            Books & publications
          </h2>
        </Reveal>
        <Reveal>
          <Card className="mt-10 grid gap-6 md:grid-cols-[1fr_2fr] md:items-center">
            <div className="rounded-md border border-border bg-foreground/[0.04] p-8 text-center">
              <span className="font-mono text-xs uppercase tracking-widest text-muted">
                O'Reilly
              </span>
              <p className="mt-4 font-mono text-3xl font-bold leading-tight">
                Extending Bazel
              </p>
              <p className="mt-2 font-mono text-xs text-muted">to its full potential</p>
            </div>
            <div>
              <div className="mb-3 flex gap-2">
                <Badge variant="outline">O'Reilly</Badge>
                <Badge variant="outline">Free</Badge>
              </div>
              <CardHeader>
                <CardTitle className="text-2xl">
                  Extending Bazel to its full potential
                </CardTitle>
              </CardHeader>
              <CardBody>
                Leveraging cloud and parallelization to ship reliable code faster.
              </CardBody>
              <div className="mt-6">
                <Button variant="link" asChild>
                  <a href="/resources/oreilly-bazel-book">Download →</a>
                </Button>
              </div>
            </div>
          </Card>
        </Reveal>
      </Section>

      {/* ANNOUNCEMENTS */}
      <Section
        width="default"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-20"
      >
        <Reveal>
          <Eyebrow className="mb-4">Announcements</Eyebrow>
          <h2 className="text-3xl font-bold leading-[1.15] md:text-4xl">From the team</h2>
          <p className="mt-4 max-w-[660px] text-base leading-relaxed text-muted md:text-lg">
            A talk from one of Trace Machina's lead engineers, Aaron Mondal, plus the
            latest news from NativeLink.
          </p>
        </Reveal>
        <Reveal>
          <div className="mt-10">
            <YouTubeEmbed
              id="uokjTev8myk"
              title="Hermetic Toolchain Creation with Local Remote Execution (LRE) & Nix"
            />
            <p className="mt-3 font-mono text-sm text-muted">
              Hermetic Toolchain Creation with Local Remote Execution (LRE) & Nix
            </p>
          </div>
        </Reveal>
      </Section>

      {/* PLACEHOLDER FOR DYNAMIC POSTS */}
      <Section
        width="default"
        className="border-t border-[rgb(var(--nl-color-accent-line))]/40 py-20"
      >
        <Reveal>
          <div className="rounded-md border-2 border-dashed border-border bg-surface/30 p-10 text-center">
            <Eyebrow className="mb-3">Coming soon</Eyebrow>
            <p className="text-lg font-medium text-foreground">
              Case studies, blog posts, and conference talks
            </p>
            <p className="mt-2 text-base text-muted">
              Migrating from the existing posts content collection in the next phase.
            </p>
          </div>
        </Reveal>
      </Section>
    </>
  );
}
