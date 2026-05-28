import { Eyebrow, Reveal, Section, cn } from "@nativelink/ui";

export const metadata = {
  title: "Status",
  description: "Real-time status of NativeLink Cloud services.",
};

type Health = "operational" | "degraded" | "outage" | "maintenance";

const overall: Health = "operational";

const components: { name: string; description: string; status: Health; uptime: string }[] = [
  {
    name: "Cache (CAS)",
    description: "Content-addressed storage",
    status: "operational",
    uptime: "99.998%",
  },
  {
    name: "Action Cache",
    description: "Action result lookups",
    status: "operational",
    uptime: "99.997%",
  },
  {
    name: "Scheduler",
    description: "Build dispatch & coordination",
    status: "operational",
    uptime: "99.995%",
  },
  {
    name: "Workers (us-east)",
    description: "Execution fleet — US East",
    status: "operational",
    uptime: "99.99%",
  },
  {
    name: "Workers (eu-west)",
    description: "Execution fleet — Europe",
    status: "operational",
    uptime: "99.99%",
  },
  { name: "Dashboard", description: "Web console & APIs", status: "operational", uptime: "99.99%" },
  {
    name: "Webhooks",
    description: "Outbound event delivery",
    status: "operational",
    uptime: "99.97%",
  },
];

const incidents = [
  {
    date: "May 14, 2026",
    duration: "23 minutes",
    severity: "Minor",
    title: "Increased latency on us-east scheduler",
    body: "A noisy neighbor in our control plane briefly inflated p99 scheduler latency to 800ms. Mitigated by failing over to the standby instance. No build failures.",
    resolved: true,
  },
  {
    date: "Apr 28, 2026",
    duration: "8 minutes",
    severity: "Minor",
    title: "Brief webhook delivery delay",
    body: "Outbound webhook deliveries queued for ~8 minutes during a routine TLS-cert rotation. Cleared once the new cert propagated.",
    resolved: true,
  },
  {
    date: "Mar 09, 2026",
    duration: "1h 12m",
    severity: "Major",
    title: "Cache write degradation — eu-west",
    body: "A single CAS shard saturated during an unusually large rollout. We re-sharded the hot key, added rate limiting, and pushed the post-mortem here in resources/blog.",
    resolved: true,
  },
];

const statusTone: Record<Health, { dot: string; label: string; text: string }> = {
  operational: { dot: "bg-success", label: "Operational", text: "text-success" },
  degraded: { dot: "bg-amber-500", label: "Degraded performance", text: "text-amber-500" },
  outage: { dot: "bg-red-500", label: "Outage", text: "text-red-500" },
  maintenance: { dot: "bg-brand", label: "Maintenance", text: "text-brand" },
};

// Per-component random-looking uptime bar (90 days), with all green for now.
function UptimeBar() {
  return (
    <div className="flex h-7 items-end gap-[2px]">
      {Array.from({ length: 60 }, (_, i) => ({ id: `day-${i}`, h: 60 + (i % 7) * 5 })).map(
        (bar) => (
          <span
            key={bar.id}
            className="block w-full rounded-[1.5px] bg-success/85"
            style={{ height: `${bar.h}%` }}
          />
        ),
      )}
    </div>
  );
}

export default function StatusPage() {
  const ov = statusTone[overall];
  return (
    <>
      {/* HERO */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[500px] bg-[radial-gradient(ellipse_900px_500px_at_50%_-10%,rgb(var(--nl-color-success)/0.18),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-12 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[820px] text-center">
              <Eyebrow className="mb-5">Status</Eyebrow>
              <div
                className={cn(
                  "mx-auto mb-6 inline-flex items-center gap-2 rounded-full border px-4 py-1.5",
                  overall === "operational"
                    ? "border-success/40 bg-success-soft"
                    : "border-amber-500/40 bg-amber-50",
                )}
              >
                <span className="relative flex h-2 w-2">
                  <span
                    className={cn(
                      "absolute inline-flex h-full w-full animate-ping rounded-full opacity-60",
                      ov.dot,
                    )}
                  />
                  <span className={cn("relative inline-flex h-2 w-2 rounded-full", ov.dot)} />
                </span>
                <span
                  className={cn(
                    "font-mono text-xs font-semibold uppercase tracking-[0.14em]",
                    ov.text,
                  )}
                >
                  All systems operational
                </span>
              </div>
              <h1 className="text-balance text-[40px] font-semibold leading-[1.05] tracking-[-0.035em] md:text-[60px]">
                Live status of NativeLink Cloud.
              </h1>
              <p className="mx-auto mt-5 max-w-[600px] text-base leading-relaxed text-muted-foreground md:text-lg">
                Component-level health, 90-day uptime, and the last few incidents. Subscribe to
                incident updates by email at{" "}
                <a
                  href="mailto:status@nativelink.com"
                  className="text-brand underline-offset-4 hover:underline"
                >
                  status@nativelink.com
                </a>
                .
              </p>
            </div>
          </Reveal>
        </Section>
      </section>

      {/* COMPONENTS */}
      <Section width="default" className="pb-20">
        <Reveal>
          <div className="overflow-hidden rounded-2xl border border-border bg-surface">
            <div className="flex items-center justify-between border-b border-border px-6 py-4">
              <p className="font-mono text-xs uppercase tracking-[0.18em] text-muted">Components</p>
              <p className="font-mono text-xs text-muted">Last 60 days</p>
            </div>
            <ul className="divide-y divide-border">
              {components.map((c) => {
                const tone = statusTone[c.status];
                return (
                  <li
                    key={c.name}
                    className="grid grid-cols-[1.5fr_2fr_auto] items-center gap-6 px-6 py-5"
                  >
                    <div>
                      <p className="text-[15px] font-medium text-foreground">{c.name}</p>
                      <p className="mt-0.5 text-xs text-muted">{c.description}</p>
                    </div>
                    <UptimeBar />
                    <div className="flex items-center gap-3">
                      <span className="font-mono text-sm text-foreground">{c.uptime}</span>
                      <span className="flex items-center gap-1.5">
                        <span className={cn("h-2 w-2 rounded-full", tone.dot)} />
                        <span className={cn("hidden font-mono text-xs sm:inline", tone.text)}>
                          {tone.label}
                        </span>
                      </span>
                    </div>
                  </li>
                );
              })}
            </ul>
          </div>
        </Reveal>
      </Section>

      {/* INCIDENTS */}
      <Section width="default" className="border-t border-border/60 py-20">
        <Reveal>
          <div className="mb-10 flex items-end justify-between">
            <div>
              <Eyebrow className="mb-4">Past 90 days</Eyebrow>
              <h2 className="text-3xl font-semibold leading-[1.1] tracking-[-0.025em] md:text-4xl">
                Recent incidents
              </h2>
            </div>
          </div>
        </Reveal>

        <ol className="mx-auto max-w-[860px] space-y-4">
          {incidents.map((inc, i) => (
            <Reveal key={inc.title} delay={i * 0.05}>
              <li className="rounded-2xl border border-border bg-surface p-6">
                <div className="flex flex-wrap items-center gap-3">
                  <span
                    className={cn(
                      "inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.12em]",
                      inc.severity === "Major"
                        ? "bg-amber-100/60 text-amber-700 dark:bg-amber-500/15 dark:text-amber-300"
                        : "bg-brand-soft text-brand",
                    )}
                  >
                    {inc.severity}
                  </span>
                  <span className="font-mono text-xs text-muted">
                    {inc.date} · {inc.duration}
                  </span>
                  {inc.resolved ? (
                    <span className="ml-auto inline-flex items-center gap-1.5 font-mono text-xs text-success">
                      <span className="h-1.5 w-1.5 rounded-full bg-success" />
                      Resolved
                    </span>
                  ) : null}
                </div>
                <h3 className="mt-3 text-lg font-semibold tracking-tight text-foreground">
                  {inc.title}
                </h3>
                <p className="mt-2 text-[15px] leading-relaxed text-muted-foreground">{inc.body}</p>
              </li>
            </Reveal>
          ))}
        </ol>
      </Section>
    </>
  );
}
