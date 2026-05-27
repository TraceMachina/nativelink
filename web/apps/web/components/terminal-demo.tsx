import { cn } from "@nativelink/ui";

interface Line {
  prompt?: boolean;
  comment?: boolean;
  info?: boolean;
  children: string;
}

interface TerminalDemoProps {
  label?: string;
  lines: Line[];
  className?: string;
}

export function TerminalDemo({
  label = "shell — quickstart",
  lines,
  className,
}: TerminalDemoProps) {
  return (
    <div
      className={cn(
        "overflow-hidden rounded-md border-2 border-border bg-foreground text-background shadow-[0_20px_60px_-15px_rgba(0,0,0,0.25)]",
        className,
      )}
    >
      <div className="flex items-center gap-2 border-b border-background/10 px-4 py-2.5">
        <span className="h-2.5 w-2.5 rounded-full bg-background/30" />
        <span className="h-2.5 w-2.5 rounded-full bg-background/30" />
        <span className="h-2.5 w-2.5 rounded-full bg-background/30" />
        <span className="ml-2 font-mono text-xs text-background/60">{label}</span>
      </div>
      <pre className="overflow-x-auto px-4 py-5 font-mono text-[13px] leading-[1.7] text-background/90">
        <code>
          {lines.map((line, i) => (
            <div
              key={`${i}-${line.children.slice(0, 8)}`}
              className={cn(
                line.comment && "text-background/45",
                line.info && "text-[#7df0c5]",
              )}
            >
              {line.prompt && <span className="text-background/40">$ </span>}
              {line.children}
            </div>
          ))}
        </code>
      </pre>
    </div>
  );
}
