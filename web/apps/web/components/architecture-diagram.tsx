"use client";

import * as motion from "motion/react-client";

export function ArchitectureDiagram() {
  return (
    <div className="relative overflow-hidden rounded-2xl border border-border bg-surface p-8">
      <div className="pointer-events-none absolute inset-0 bg-dot-grid opacity-40 [mask-image:radial-gradient(ellipse_at_center,black_30%,transparent_75%)]" />

      <svg
        viewBox="0 0 800 360"
        className="relative h-auto w-full"
        role="img"
        aria-label="NativeLink architecture diagram"
      >
        <title>
          NativeLink architecture: client to scheduler to worker fleet, backed by CAS and AC
        </title>
        <defs>
          <linearGradient id="arc-grad" x1="0" y1="0" x2="800" y2="0">
            <stop offset="0%" stopColor="rgb(var(--nl-color-brand))" stopOpacity="0.0" />
            <stop offset="50%" stopColor="rgb(var(--nl-color-brand))" stopOpacity="0.8" />
            <stop offset="100%" stopColor="rgb(var(--nl-color-brand))" stopOpacity="0.0" />
          </linearGradient>
          <linearGradient id="node-grad" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="rgb(var(--nl-color-surface-elevated))" />
            <stop offset="100%" stopColor="rgb(var(--nl-color-surface))" />
          </linearGradient>
        </defs>

        {/* Connection lines */}
        {[
          { d: "M 150 180 C 250 180, 250 80, 360 80" },
          { d: "M 150 180 C 250 180, 250 180, 360 180" },
          { d: "M 150 180 C 250 180, 250 280, 360 280" },
          { d: "M 520 80 C 600 80, 600 180, 670 180" },
          { d: "M 520 180 C 600 180, 600 180, 670 180" },
          { d: "M 520 280 C 600 280, 600 180, 670 180" },
        ].map((path, i) => (
          <g key={path.d}>
            <path
              d={path.d}
              fill="none"
              stroke="rgb(var(--nl-color-border-strong))"
              strokeWidth="1.5"
              strokeDasharray="4 4"
            />
            <motion.circle
              r="4"
              fill="rgb(var(--nl-color-brand))"
              initial={{ pathOffset: 0 }}
              animate={{ pathOffset: 1 }}
              transition={{
                duration: 2 + i * 0.2,
                repeat: Number.POSITIVE_INFINITY,
                delay: i * 0.4,
              }}
            >
              <animateMotion dur={`${2 + i * 0.2}s`} repeatCount="indefinite" path={path.d} />
            </motion.circle>
          </g>
        ))}

        {/* Client node */}
        <g>
          <rect
            x="40"
            y="140"
            width="120"
            height="80"
            rx="12"
            fill="url(#node-grad)"
            stroke="rgb(var(--nl-color-border))"
            strokeWidth="1.5"
          />
          <text
            x="100"
            y="172"
            textAnchor="middle"
            fontFamily="var(--nl-font-mono)"
            fontSize="11"
            fontWeight="600"
            fill="rgb(var(--nl-color-foreground))"
          >
            CLIENT
          </text>
          <text
            x="100"
            y="190"
            textAnchor="middle"
            fontFamily="var(--nl-font-sans)"
            fontSize="10"
            fill="rgb(var(--nl-color-muted))"
          >
            Bazel · Buck2
          </text>
          <text
            x="100"
            y="204"
            textAnchor="middle"
            fontFamily="var(--nl-font-sans)"
            fontSize="10"
            fill="rgb(var(--nl-color-muted))"
          >
            Reclient · Pants
          </text>
        </g>

        {/* Middle layer — NativeLink (3 nodes) */}
        {[
          { y: 40, label: "SCHEDULER", subA: "Action dispatch", subB: "Worker assign" },
          { y: 140, label: "CAS", subA: "Content-addressed", subB: "Dedup & cache" },
          { y: 240, label: "AC", subA: "Action results", subB: "Provenance" },
        ].map((node) => (
          <g key={node.label}>
            <rect
              x="360"
              y={node.y}
              width="160"
              height="80"
              rx="12"
              fill="rgb(var(--nl-color-brand-soft))"
              stroke="rgb(var(--nl-color-brand) / 0.5)"
              strokeWidth="1.5"
            />
            <text
              x="440"
              y={node.y + 30}
              textAnchor="middle"
              fontFamily="var(--nl-font-mono)"
              fontSize="11"
              fontWeight="700"
              fill="rgb(var(--nl-color-brand-strong))"
            >
              {node.label}
            </text>
            <text
              x="440"
              y={node.y + 48}
              textAnchor="middle"
              fontFamily="var(--nl-font-sans)"
              fontSize="10"
              fill="rgb(var(--nl-color-muted-foreground))"
            >
              {node.subA}
            </text>
            <text
              x="440"
              y={node.y + 62}
              textAnchor="middle"
              fontFamily="var(--nl-font-sans)"
              fontSize="10"
              fill="rgb(var(--nl-color-muted-foreground))"
            >
              {node.subB}
            </text>
          </g>
        ))}

        {/* Workers */}
        <g>
          <rect
            x="670"
            y="140"
            width="120"
            height="80"
            rx="12"
            fill="url(#node-grad)"
            stroke="rgb(var(--nl-color-border))"
            strokeWidth="1.5"
          />
          <text
            x="730"
            y="172"
            textAnchor="middle"
            fontFamily="var(--nl-font-mono)"
            fontSize="11"
            fontWeight="600"
            fill="rgb(var(--nl-color-foreground))"
          >
            WORKER FLEET
          </text>
          <text
            x="730"
            y="190"
            textAnchor="middle"
            fontFamily="var(--nl-font-sans)"
            fontSize="10"
            fill="rgb(var(--nl-color-muted))"
          >
            x86 · ARM · GPU
          </text>
          <text
            x="730"
            y="204"
            textAnchor="middle"
            fontFamily="var(--nl-font-sans)"
            fontSize="10"
            fill="rgb(var(--nl-color-muted))"
          >
            AWS · GCP · Bare
          </text>
        </g>

        {/* Container labels */}
        <text
          x="100"
          y="120"
          textAnchor="middle"
          fontFamily="var(--nl-font-mono)"
          fontSize="9"
          letterSpacing="2"
          fill="rgb(var(--nl-color-muted))"
        >
          YOUR CI
        </text>
        <text
          x="440"
          y="20"
          textAnchor="middle"
          fontFamily="var(--nl-font-mono)"
          fontSize="9"
          letterSpacing="2"
          fill="rgb(var(--nl-color-brand))"
        >
          NATIVELINK
        </text>
        <text
          x="730"
          y="120"
          textAnchor="middle"
          fontFamily="var(--nl-font-mono)"
          fontSize="9"
          letterSpacing="2"
          fill="rgb(var(--nl-color-muted))"
        >
          ANY CLOUD
        </text>
      </svg>
    </div>
  );
}
