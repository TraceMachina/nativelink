import Link from "next/link";

export default function HomePage() {
  return (
    <main className="flex flex-1 flex-col justify-center px-6 py-24 text-center">
      <h1 className="mb-6 text-4xl font-bold leading-[1.15] tracking-tight md:text-5xl">
        NativeLink Documentation
      </h1>
      <p className="mx-auto mb-10 max-w-[600px] text-lg text-muted">
        Configuration reference, deployment guides, and architecture for the NativeLink
        remote build cache and execution platform.
      </p>
      <div>
        <Link
          href="/docs"
          className="inline-flex h-12 items-center justify-center rounded-md border-2 border-foreground bg-foreground px-8 font-mono text-base text-background hover:bg-[rgb(var(--nl-color-accent-hover))]"
        >
          Open docs
        </Link>
      </div>
    </main>
  );
}
