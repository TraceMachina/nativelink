import {
  Badge,
  Button,
  Card,
  CardBody,
  CardFooter,
  CardHeader,
  CardTitle,
  Divider,
  Logo,
  Prose,
  Section,
} from "@nativelink/ui";
import type { Metadata } from "next";
import { notFound } from "next/navigation";

export const metadata: Metadata = {
  title: "UI Lab",
  robots: { index: false, follow: false },
};

// Dev-only: the design-system catalog has no place in the production bundle.
const IS_PROD = process.env.NODE_ENV === "production";

function Group({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <section className="mb-16">
      <h2 className="mb-2 font-mono text-xs uppercase tracking-widest text-muted">{label}</h2>
      <Divider className="mb-8" />
      <div className="flex flex-wrap items-center gap-4">{children}</div>
    </section>
  );
}

export default function LabPage() {
  if (IS_PROD) notFound();
  return (
    <Section width="default" className="pt-16 pb-32">
      <header className="mb-12">
        <Badge variant="solid" className="mb-4">
          internal
        </Badge>
        <h1 className="text-4xl font-bold leading-[1.15] tracking-tight md:text-5xl">
          Design system lab
        </h1>
        <p className="mt-3 max-w-[60ch] text-base text-muted">
          Every primitive in <code>@nativelink/ui</code>, in every meaningful state.
          The living reference for the "Technical Warmth" system.
        </p>
      </header>

      <Group label="Logo">
        <Logo size="sm" />
        <Logo size="md" />
        <Logo size="lg" />
      </Group>

      <Group label="Buttons — variants">
        <Button>Primary</Button>
        <Button variant="outline">Outline</Button>
        <Button variant="ghost">Ghost</Button>
        <Button variant="link">Link</Button>
      </Group>

      <Group label="Buttons — sizes">
        <Button size="sm">Small</Button>
        <Button size="md">Medium</Button>
        <Button size="lg">Large</Button>
        <Button disabled>Disabled</Button>
      </Group>

      <Group label="Badges">
        <Badge>Default</Badge>
        <Badge variant="solid">Solid</Badge>
        <Badge variant="outline">Outline</Badge>
        <Badge variant="brand">Brand</Badge>
        <Badge variant="success">Success</Badge>
      </Group>

      <Group label="Cards">
        <div className="grid w-full gap-6 md:grid-cols-3">
          <Card>
            <CardHeader>
              <CardTitle>Basic card</CardTitle>
            </CardHeader>
            <CardBody>
              Standard card with header, body, and a 2px border. Semi-transparent surface
              over the warm background.
            </CardBody>
          </Card>
          <Card>
            <CardHeader>
              <CardTitle>With footer</CardTitle>
            </CardHeader>
            <CardBody>
              Cards compose with a footer slot for action rows or metadata.
            </CardBody>
            <CardFooter>
              <Button size="sm">Action</Button>
              <Button size="sm" variant="ghost">
                Dismiss
              </Button>
            </CardFooter>
          </Card>
          <Card>
            <CardHeader>
              <CardTitle>Dense</CardTitle>
            </CardHeader>
            <CardBody>
              Same card, more content. Body text uses the muted foreground color and
              relaxed line-height for long-form reading.
            </CardBody>
          </Card>
        </div>
      </Group>

      <Group label="Dividers">
        <div className="w-full space-y-8">
          <div>
            <p className="mb-2 text-sm text-muted">Full-width</p>
            <Divider />
          </div>
          <div>
            <p className="mb-2 text-sm text-muted">Inset 10% (matches Section divider)</p>
            <Divider inset />
          </div>
        </div>
      </Group>

      <Group label="Prose">
        <Prose className="w-full">
          <h1>Heading 1</h1>
          <h2>Heading 2</h2>
          <h3>Heading 3</h3>
          <p>
            Body paragraphs render at <code>16px</code> with a <code>1.7</code> line-height
            for comfortable reading. Inline code uses a subtle background tint.
          </p>
          <pre>
            <code>{`function add(a: number, b: number) {\n  return a + b;\n}`}</code>
          </pre>
          <ul>
            <li>Unordered lists use disc markers.</li>
            <li>Spacing follows the 8px rhythm.</li>
          </ul>
          <blockquote>
            Quotes use a 2px left border and muted color. They lean into the technical-but-warm tone.
          </blockquote>
        </Prose>
      </Group>

      <Group label="Color tokens">
        <div className="grid w-full grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-4">
          {[
            ["background", "--nl-color-background"],
            ["foreground", "--nl-color-foreground"],
            ["muted", "--nl-color-muted"],
            ["muted-fg", "--nl-color-muted-foreground"],
            ["border", "--nl-color-border"],
            ["surface", "--nl-color-surface"],
            ["accent", "--nl-color-accent"],
            ["accent-line", "--nl-color-accent-line"],
          ].map(([name, varName]) => (
            <div
              key={name}
              className="overflow-hidden rounded-md border-2 border-border bg-surface/50"
            >
              <div className="h-16 w-full" style={{ background: `rgb(var(${varName}))` }} />
              <div className="border-t border-border px-3 py-2">
                <div className="font-mono text-sm font-medium">{name}</div>
                <div className="font-mono text-xs text-muted">{varName}</div>
              </div>
            </div>
          ))}
        </div>
      </Group>
    </Section>
  );
}
