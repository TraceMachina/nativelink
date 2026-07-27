import { Badge, Eyebrow, Reveal, Section } from "@nativelink/ui";
import type { Post } from "../../lib/posts";
import { formatPostDate } from "../../lib/posts";

export interface PostCardData {
  key: string;
  href: string;
  external?: boolean;
  tags: string[];
  meta: string;
  title: string;
  excerpt: string;
  cta: string;
}

export function postToCard(post: Post): PostCardData {
  return {
    key: post.slug,
    href: `/resources/blog/${encodeURIComponent(post.slug)}`,
    tags: post.tags,
    meta: `${formatPostDate(post.pubDate)}${post.readTime ? ` · ${post.readTime}` : ""}`,
    title: post.title,
    excerpt: post.excerpt,
    cta: "Read post",
  };
}

export function PostCard({ card, index }: { card: PostCardData; index: number }) {
  return (
    <Reveal delay={(index % 2) * 0.04}>
      <a
        href={card.href}
        {...(card.external ? { target: "_blank", rel: "noreferrer" } : {})}
        className="group flex h-full flex-col justify-between rounded-2xl border border-border bg-surface p-7 transition-all hover:border-brand/40 hover:shadow-[0_20px_50px_-25px_rgb(var(--nl-color-brand)/0.35)]"
      >
        <div>
          <div className="mb-4 flex flex-wrap items-center gap-2">
            {card.tags.map((tag) => (
              <Badge key={tag} variant="outline">
                {tag}
              </Badge>
            ))}
            <span className="font-mono text-xs text-muted">{card.meta}</span>
          </div>
          <h3 className="text-xl font-semibold leading-tight tracking-tight text-foreground">
            {card.title}
          </h3>
          <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">{card.excerpt}</p>
        </div>
        <div className="mt-6 inline-flex items-center gap-1.5 font-mono text-sm text-brand">
          {card.cta}{" "}
          <span aria-hidden="true" className="transition-transform group-hover:translate-x-1">
            →
          </span>
        </div>
      </a>
    </Reveal>
  );
}

export function PostSection({
  title,
  cards,
  className,
}: {
  title: string;
  cards: PostCardData[];
  className?: string;
}) {
  if (cards.length === 0) {
    return null;
  }
  return (
    <Section width="default" className={className}>
      <Reveal>
        <Eyebrow className="mb-8">{title}</Eyebrow>
      </Reveal>
      <div className="grid gap-5 md:grid-cols-2">
        {cards.map((card, i) => (
          <PostCard key={card.key} card={card} index={i} />
        ))}
      </div>
    </Section>
  );
}
