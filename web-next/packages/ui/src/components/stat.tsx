import * as React from "react";
import { cn } from "../lib/cn";

interface StatProps extends React.HTMLAttributes<HTMLDivElement> {
  value: React.ReactNode;
  label: React.ReactNode;
}

export function Stat({ value, label, className, ...props }: StatProps) {
  return (
    <div className={cn("flex flex-col gap-2", className)} {...props}>
      <div className="font-mono text-3xl font-bold leading-none tracking-tight md:text-4xl">
        {value}
      </div>
      <div className="font-mono text-sm text-muted">{label}</div>
    </div>
  );
}

export function StatGrid({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "grid grid-cols-2 gap-x-8 gap-y-10 md:grid-cols-4",
        className,
      )}
      {...props}
    />
  );
}
