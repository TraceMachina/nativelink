import { cn } from "../lib/cn";

interface VideoEmbedProps {
  src: string;
  title?: string;
  className?: string;
  aspect?: "16/9" | "4/3" | "1/1";
}

const aspectMap = {
  "16/9": "aspect-video",
  "4/3": "aspect-[4/3]",
  "1/1": "aspect-square",
} as const;

export function VideoEmbed({
  src,
  title = "Video",
  className,
  aspect = "16/9",
}: VideoEmbedProps) {
  return (
    <div
      className={cn(
        "relative w-full overflow-hidden rounded-md border-2 border-border bg-foreground/5",
        aspectMap[aspect],
        className,
      )}
    >
      <iframe
        src={src}
        title={title}
        loading="lazy"
        allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture; web-share"
        allowFullScreen
        className="absolute inset-0 h-full w-full"
      />
    </div>
  );
}

export function YouTubeEmbed({
  id,
  title,
  className,
}: {
  id: string;
  title?: string;
  className?: string;
}) {
  return (
    <VideoEmbed
      src={`https://www.youtube-nocookie.com/embed/${id}`}
      title={title ?? "YouTube video"}
      className={className}
    />
  );
}
