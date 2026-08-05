# `web` — nativelink.com

The marketing site, docs, and shared design system that ship to
[nativelink.com](https://nativelink.com).

## Layout

```
web/
├── apps/
│   ├── web/        Next.js 15 marketing site (nativelink.com)
│   └── docs/       Next.js 15 + Fumadocs (nativelink.com/docs)
└── packages/
    ├── tokens/     Design tokens (CSS custom properties)
    ├── ui/         shadcn-based React component library
    └── config/     Shared tsconfig, biome, Tailwind preset
```

## Stack

- **Next.js 15** App Router + React 19
- **Tailwind v4** (CSS-first config, brand tokens in `@nativelink/tokens`)
- **Fumadocs 15** for the docs (MDX, Orama search, Shiki code blocks)
- **Geist** Sans + Mono via `next/font`
- **Biome** for lint/format
- **Bun** + **Turborepo** for the monorepo
- **Vercel** as the deployment target

## Prerequisites

- **Bun** 1.2+ — `curl -fsSL https://bun.sh/install | bash`
- **Node.js** 20+ (Bun installs and uses its own; the engine field
  enforces a minimum for tools that shell out)

## Develop

```bash
# Install once at the workspace root
cd web
bun install

# Run both apps in parallel via Turborepo
bun dev
#   marketing → http://localhost:3000
#   docs      → http://localhost:3001/docs
#   marketing rewrites /docs/* to the docs server so you can browse
#   both apps from http://localhost:3000

# Or run a single app
bun dev:web    # marketing only
bun dev:docs   # docs only
```

Stop everything:

```bash
lsof -i :3000 -i :3001 | awk 'NR>1 {print $2}' | sort -u | xargs kill -9
```

(`pkill -f "next dev"` doesn't catch processes spawned via Bun.)

## Build

```bash
# Build everything via Turborepo
bun build

# Or build one app
bun --filter @nativelink/web build
bun --filter @nativelink/docs build
```

Build outputs land in each app's `.next/` directory. Both apps prerender
as much as possible at build time:

- **`apps/web`** ships 12 static routes (~157 kB First Load JS) plus a
  dynamic OG-image route.
- **`apps/docs`** ships 33 prerendered MDX pages, a dynamic search API,
  and a dynamic OG-image route.

## Typecheck & lint

```bash
bun typecheck   # tsc --noEmit on every workspace
bun check       # biome lint
bun format      # biome format --write
```

CI runs `bun typecheck` and `bun check`; both must pass.

## Components & design tokens

The brand system lives in `packages/`. The full component catalog is at
`/lab` in the marketing app (dev-only — gated on `NODE_ENV` so it 404s
in production):

```bash
bun dev:web
# open http://localhost:3000/lab
```

Tokens are CSS custom properties exported from `@nativelink/tokens`. The
shared Tailwind preset (`@nativelink/config/tailwind/preset.css`) maps
them to Tailwind's theme so both apps inherit the same palette in light
and dark mode.

## Editing docs

Pages are MDX, one file per page, under `apps/docs/content/docs/`. The
sidebar order is controlled by per-folder `meta.json` files; section
dividers in the sidebar come from `---Title---` entries in the top-level
`meta.json`.

Custom MDX components available without import:

- `<Callout type="info|warn|success|error" title="...">` — themed asides
- `<Steps>` with `<li>` children — numbered tutorial lists
- `<Tabs items={[...]}> <Tab value="...">` — tabbed content
- `<Mermaid>{` …diagram source… `}</Mermaid>` — themed diagrams

Full conventions live at
[`/docs/contribute/docs`](https://nativelink.com/docs/contribute/docs).

## Deploy

Two Vercel projects, one repo. The marketing app proxies `/docs/*` to
the docs deployment via a Next.js rewrite that reads the `DOCS_URL`
env var. Full walkthrough in [`DEPLOYMENT.md`](./DEPLOYMENT.md).

## License

Same as the parent repository (`FSL-1.1-Apache-2.0`).
