import { Badge, Eyebrow, Prose, Section } from "@nativelink/ui";
import { marked } from "marked";
import { notFound } from "next/navigation";
import { formatPostDate, getAllPosts, getPost } from "../../../../lib/posts";

export function generateStaticParams() {
  return getAllPosts().map((post) => ({ slug: post.slug }));
}

export async function generateMetadata({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const post = getPost((await params).slug);
  if (!post) return {};
  return {
    title: post.title,
    description: post.excerpt,
    openGraph: {
      title: post.title,
      description: post.excerpt,
      type: "article",
      ...(post.image ? { images: [{ url: post.image }] } : {}),
    },
  };
}

export default async function BlogPostPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const post = getPost((await params).slug);
  if (!post) notFound();

  const html = marked.parse(post.body, { async: false });

  return (
    <>
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[400px] bg-[radial-gradient(ellipse_900px_400px_at_50%_-10%,rgb(var(--nl-color-brand)/0.10),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-10 md:pt-32">
          <div className="mx-auto max-w-[760px]">
            <a
              href="/resources/blog"
              className="font-mono text-sm text-brand transition-colors hover:text-foreground"
            >
              ← All posts
            </a>
            <div className="mt-8 flex flex-wrap items-center gap-2">
              {post.tags.map((tag) => (
                <Badge key={tag} variant="outline">
                  {tag}
                </Badge>
              ))}
            </div>
            <h1 className="mt-5 text-balance text-[34px] font-semibold leading-[1.1] tracking-[-0.03em] md:text-[46px]">
              {post.title}
            </h1>
            <p className="mt-5 font-mono text-sm text-muted">
              {formatPostDate(post.pubDate)}
              {post.readTime ? ` · ${post.readTime}` : null}
            </p>
          </div>
        </Section>
      </section>

      <Section width="default" className="pb-28">
        <div className="mx-auto max-w-[760px]">
          <Prose
            className="max-w-none [&_img]:my-6 [&_img]:max-w-full [&_img]:rounded-xl"
            // biome-ignore lint/security/noDangerouslySetInnerHtml: Post bodies are trusted repo content, rendered from markdown at build time.
            dangerouslySetInnerHTML={{ __html: html }}
          />
          <div className="mt-16 border-t border-border pt-8">
            <Eyebrow tone="muted" className="text-[10px]">
              NativeLink Blog
            </Eyebrow>
            <p className="mt-3 text-sm text-muted-foreground">
              <a
                href="/resources/blog"
                className="text-brand transition-colors hover:text-foreground"
              >
                ← Back to all posts
              </a>
            </p>
          </div>
        </div>
      </Section>
    </>
  );
}
