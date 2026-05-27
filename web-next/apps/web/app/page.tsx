import { Badge, Button, Card, CardBody, CardHeader, CardTitle, Section } from "@nativelink/ui";

export default function HomePage() {
  return (
    <>
      <Section width="narrow" className="pt-[120px] pb-20 text-center">
        <Badge variant="outline" className="mb-6">
          web-next · phase 2
        </Badge>
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
          <Button size="lg" asChild>
            <a href="/docs">Get started</a>
          </Button>
          <Button size="lg" variant="outline" asChild>
            <a href="/product">View product</a>
          </Button>
        </div>
      </Section>

      <Section divider className="border-t border-[rgb(var(--nl-color-accent-line))]/40">
        <h2 className="mb-12 text-center text-3xl font-bold md:text-4xl">
          Phase 2 — design system primitives
        </h2>
        <div className="grid gap-6 md:grid-cols-3">
          <Card>
            <CardHeader>
              <CardTitle>Header & footer</CardTitle>
            </CardHeader>
            <CardBody>
              Site chrome ships with primary nav, GitHub link, CTA, and a three-column
              footer with brand mark and contact links.
            </CardBody>
          </Card>
          <Card>
            <CardHeader>
              <CardTitle>Logo + Badge + Divider</CardTitle>
            </CardHeader>
            <CardBody>
              Three more primitives backed by the same token system. Browse the full
              catalog at <a className="underline" href="/lab">/lab</a>.
            </CardBody>
          </Card>
          <Card>
            <CardHeader>
              <CardTitle>Coming in phase 3</CardTitle>
            </CardHeader>
            <CardBody>
              Marketing page ports — Home, Product, Pricing, Company, Resources,
              Community — with a light copy refresh and Motion-powered entrance
              animations.
            </CardBody>
          </Card>
        </div>
      </Section>
    </>
  );
}
