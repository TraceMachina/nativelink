import { Button, Card, CardBody, CardHeader, CardTitle, Section } from "@nativelink/ui";

export default function HomePage() {
  return (
    <main className="min-h-screen">
      <Section width="narrow" className="pt-[120px] pb-20 text-center">
        <p className="mb-6 font-mono text-sm uppercase tracking-widest text-muted">
          web-next scaffold
        </p>
        <h1 className="text-5xl font-bold leading-[1.15] tracking-tight md:text-6xl">
          When agents write your code,
          <br />
          your build system is the bottleneck.
        </h1>
        <p className="mx-auto mt-8 max-w-[550px] text-lg leading-relaxed text-muted">
          NativeLink is the high-performance remote build cache and execution platform
          for Bazel and beyond. Cut CI by 90%, ship 10× faster.
        </p>
        <div className="mt-10 flex flex-wrap justify-center gap-4">
          <Button size="lg">Get started</Button>
          <Button size="lg" variant="outline">
            View docs
          </Button>
        </div>
      </Section>

      <Section divider className="border-t border-[rgb(var(--nl-color-accent-line))]/40">
        <h2 className="mb-12 text-center text-3xl font-bold md:text-4xl">
          Phase 1 scaffold checklist
        </h2>
        <div className="grid gap-6 md:grid-cols-3">
          <Card>
            <CardHeader>
              <CardTitle>Design tokens</CardTitle>
            </CardHeader>
            <CardBody>
              Ported from DESIGN.md into <code>@nativelink/tokens</code>. Tailwind v4 reads
              every var via the preset.
            </CardBody>
          </Card>
          <Card>
            <CardHeader>
              <CardTitle>UI primitives</CardTitle>
            </CardHeader>
            <CardBody>
              Button, Card, Section, Prose in <code>@nativelink/ui</code>. shadcn-style,
              theme-driven, accessible.
            </CardBody>
          </Card>
          <Card>
            <CardHeader>
              <CardTitle>Two apps, one repo</CardTitle>
            </CardHeader>
            <CardBody>
              <code>apps/web</code> for marketing, <code>apps/docs</code> for Fumadocs.
              Turborepo orchestrates.
            </CardBody>
          </Card>
        </div>
      </Section>
    </main>
  );
}
