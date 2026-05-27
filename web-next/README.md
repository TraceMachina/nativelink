# NativeLink web-next

Modern rebuild of nativelink.com. Replaces `web/platform/`.

## Layout

```
web-next/
├── apps/
│   ├── web/        Next.js 15 marketing site (nativelink.com)
│   └── docs/       Next.js 15 + Fumadocs (nativelink.com/docs)
└── packages/
    ├── tokens/     Design tokens (CSS custom properties)
    ├── ui/         shadcn-based component library
    └── config/     Shared tsconfig, biome, Tailwind preset
```

## Stack

- **Next.js 15** (App Router, RSC)
- **React 19**
- **Tailwind v4** (CSS-first config)
- **Fumadocs** for docs
- **Biome** for lint/format
- **Bun** + **Turborepo** for the monorepo
- **Vercel** for deployment

## Develop

```bash
# from web-next/
bun install
bun dev                 # both apps
bun dev:web             # marketing only (http://localhost:3000)
bun dev:docs            # docs only (http://localhost:3001)
```

## Build

```bash
bun build
bun typecheck
bun check               # biome lint
```

## Status

Phase 1 scaffold. No content ported yet. See plan in conversation history.
