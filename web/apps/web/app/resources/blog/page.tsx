import { Eyebrow, Reveal, Section } from "@nativelink/ui";
import { getAllPosts } from "../../../lib/posts";
import { PostSection, postToCard } from "../post-sections";

export const metadata = { title: "Blog" };

export default function BlogIndexPage() {
  const posts = getAllPosts();
  const caseStudies = posts.filter((p) => p.tags.includes("case-studies"));
  const announcements = posts.filter(
    (p) => p.tags.includes("announcements") && !p.tags.includes("case-studies"),
  );
  const others = posts.filter(
    (p) => !p.tags.includes("case-studies") && !p.tags.includes("announcements"),
  );

  return (
    <>
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[400px] bg-[radial-gradient(ellipse_900px_400px_at_50%_-10%,rgb(var(--nl-color-brand)/0.13),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-14 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[820px] text-center">
              <Eyebrow className="mb-5">Blog</Eyebrow>
              <h1 className="text-balance text-[40px] font-semibold leading-[1.05] tracking-[-0.04em] md:text-[56px]">
                Writing from the team
              </h1>
              <p className="mx-auto mt-5 max-w-[640px] text-[17px] leading-relaxed text-muted-foreground">
                Tutorials, case studies, and announcements from the people building NativeLink.
              </p>
            </div>
          </Reveal>
        </Section>
      </section>

      <PostSection title="Case studies" cards={caseStudies.map(postToCard)} className="pb-16" />
      <PostSection
        title="Announcements"
        cards={announcements.map(postToCard)}
        className="border-t border-border/60 pt-16 pb-16"
      />
      <PostSection
        title="Blog"
        cards={others.map(postToCard)}
        className="border-t border-border/60 pt-16 pb-28"
      />
    </>
  );
}
