import { BookDownloadForm } from "@/components/book-download-form";
import { Eyebrow, Reveal, Section } from "@nativelink/ui";
import type { Metadata } from "next";
import Image from "next/image";

export const metadata: Metadata = {
  title: "Extending Bazel to Its Full Potential — O'Reilly Book",
  description:
    "Download the free O'Reilly e-book Extending Bazel to Its Full Potential — cloud, parallelization, and NativeLink for faster reliable builds.",
};

const highlights = [
  "Cloud-native build infrastructure design",
  "Advanced parallelization strategies",
  "Remote execution and caching best practices",
  "New Bazel use cases and implementations",
];

const features = [
  {
    title: "Comprehensive intro",
    body: "From basics to advanced topics in distributed builds.",
    icon: (
      <path
        strokeLinecap="round"
        strokeLinejoin="round"
        d="M12 6.253v13m0-13C10.832 5.477 9.246 5 7.5 5S4.168 5.477 3 6.253v13C4.168 18.477 5.754 18 7.5 18s3.332.477 4.5 1.253m0-13C13.168 5.477 14.754 5 16.5 5c1.747 0 3.332.477 4.5 1.253v13C19.832 18.477 18.247 18 16.5 18c-1.746 0-3.332.477-4.5 1.253"
      />
    ),
  },
  {
    title: "Practical examples",
    body: "Real-world code samples and configurations.",
    icon: (
      <path
        strokeLinecap="round"
        strokeLinejoin="round"
        d="M10 20l4-16m4 4l4 4-4 4M6 16l-4-4 4-4"
      />
    ),
  },
  {
    title: "Performance tips",
    body: "Optimize your builds for maximum efficiency.",
    icon: <path strokeLinecap="round" strokeLinejoin="round" d="M13 10V3L4 14h7v7l9-11h-7z" />,
  },
];

export default function OreillyBazelBookPage() {
  return (
    <>
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[500px] bg-[radial-gradient(ellipse_900px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.13),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-16 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[820px] text-center">
              <Eyebrow className="mb-5">Free O'Reilly e-book</Eyebrow>
              <h1 className="text-balance text-[40px] font-semibold leading-[1.05] tracking-[-0.04em] md:text-6xl">
                Extending Bazel to Its Full Potential
              </h1>
              <p className="mx-auto mt-6 max-w-[640px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                Leveraging cloud and parallelization to ship reliable code faster.
              </p>
            </div>
          </Reveal>
        </Section>
      </section>

      <Section width="default" className="pb-20">
        <Reveal>
          <div className="overflow-hidden rounded-3xl border border-border bg-surface">
            <div className="grid items-center gap-10 p-8 md:grid-cols-2 md:gap-12 md:p-12">
              <div>
                <h2 className="text-balance text-3xl font-semibold leading-tight tracking-[-0.02em] text-foreground md:text-4xl">
                  Master distributed builds at scale
                </h2>
                <p className="mt-5 text-base leading-relaxed text-muted-foreground md:text-lg">
                  Discover how to extend Bazel beyond its traditional boundaries. This comprehensive
                  guide from O'Reilly Media explores advanced techniques for leveraging cloud
                  infrastructure and parallelization to dramatically improve your build performance
                  and reliability.
                </p>
                <ul className="mt-8 space-y-3">
                  {highlights.map((item) => (
                    <li key={item} className="flex items-start gap-3 text-[15px] text-foreground">
                      <span
                        aria-hidden="true"
                        className="mt-0.5 inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-brand-soft font-mono text-xs text-brand"
                      >
                        ✓
                      </span>
                      <span>{item}</span>
                    </li>
                  ))}
                </ul>
              </div>
              <div className="flex flex-col items-center">
                <div className="overflow-hidden rounded-2xl border border-border shadow-[0_30px_80px_-40px_rgb(0_0_0_/_0.45)]">
                  <Image
                    src="/assets/oreilly-bazel-book-cover.png"
                    alt="Extending Bazel to Its Full Potential — O'Reilly book cover"
                    width={876}
                    height={654}
                    className="h-auto w-full"
                    priority
                  />
                </div>
                <p className="mt-4 font-mono text-xs text-muted">Published by O'Reilly Media</p>
              </div>
            </div>
          </div>
        </Reveal>
      </Section>

      <Section width="default" className="border-t border-border/60 bg-surface-elevated/40 py-20">
        <Reveal>
          <BookDownloadForm />
        </Reveal>
      </Section>

      <Section width="default" className="py-20">
        <div className="grid gap-5 md:grid-cols-3">
          {features.map((feature, index) => (
            <Reveal key={feature.title} delay={index * 0.06}>
              <article className="flex h-full flex-col items-center rounded-2xl border border-border bg-surface p-8 text-center">
                <span className="mb-6 inline-flex h-14 w-14 items-center justify-center rounded-2xl bg-brand-soft text-brand">
                  <svg
                    width="28"
                    height="28"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="1.8"
                    aria-hidden="true"
                  >
                    {feature.icon}
                  </svg>
                </span>
                <h3 className="text-xl font-semibold tracking-tight text-foreground">
                  {feature.title}
                </h3>
                <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
                  {feature.body}
                </p>
              </article>
            </Reveal>
          ))}
        </div>
      </Section>
    </>
  );
}
