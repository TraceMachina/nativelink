import { Badge, Eyebrow, Reveal, Section } from "@nativelink/ui";
import { type Post, formatPostDate, getAllPosts } from "../../../lib/posts";

export const metadata = { title: "Blog" };

function PostCard({ post, index }: { post: Post; index: number }) {
  return (
    <Reveal key={post.slug} delay={(index % 2) * 0.04}>
      <a
        href={`/resources/blog/${encodeURIComponent(post.slug)}`}
        className="group flex h-full flex-col justify-between rounded-2xl border border-border bg-surface p-7 transition-all hover:border-brand/40 hover:shadow-[0_20px_50px_-25px_rgb(var(--nl-color-brand)/0.35)]"
      >
        <div>
          <div className="mb-4 flex flex-wrap items-center gap-2">
            {post.tags.map((tag) => (
              <Badge key={tag} variant="outline">
                {tag}
              </Badge>
            ))}
            <span className="font-mono text-xs text-muted">
              {formatPostDate(post.pubDate)}
              {post.readTime ? ` · ${post.readTime}` : null}
            </span>
          </div>
          <h2 className="text-xl font-semibold leading-tight tracking-tight text-foreground">
            {post.title}
          </h2>
          <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">{post.excerpt}</p>
        </div>
        <div className="mt-6 inline-flex items-center gap-1.5 font-mono text-sm text-brand">
          Read post{" "}
          <span aria-hidden="true" className="transition-transform group-hover:translate-x-1">
            →
          </span>
        </div>
      </a>
    </Reveal>
  );
}

function PostSection({
  title,
  posts,
  className,
}: {
  title: string;
  posts: Post[];
  className?: string;
}) {
  if (posts.length === 0) {
    return null;
  }
  return (
    <Section width="default" className={className}>
      <Reveal>
        <Eyebrow className="mb-8">{title}</Eyebrow>
      </Reveal>
      <div className="grid gap-5 md:grid-cols-2">
        {posts.map((post, i) => (
          <PostCard key={post.slug} post={post} index={i} />
        ))}
      </div>
    </Section>
  );
}

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

      <PostSection title="Case studies" posts={caseStudies} className="pb-16" />
      <PostSection
        title="Announcements"
        posts={announcements}
        className="border-t border-border/60 pt-16 pb-16"
      />
      <PostSection
        title="More from the blog"
        posts={others}
        className="border-t border-border/60 pt-16 pb-28"
      />
    </>
  );
}
