# Deploying `web`

Two Vercel projects, one repo. The marketing app at the apex
(`nativelink.com`) proxies `/docs/*` to the docs app via a Next.js
rewrite. This page walks through the setup end-to-end.

## Topology

```
                  nativelink.com
                       │
        ┌──────────────┴──────────────┐
        │                             │
        ▼                             ▼
  Marketing project              Docs project
  (apps/web)                     (apps/docs)
  Vercel rewrites /docs/* ──────▶ Next basePath "/docs"
```

Both projects build from the same monorepo. Vercel auto-detects the
Turborepo + Bun setup; the only project-level setting that matters is
**Root Directory**.

## One-time setup

### 1. Create the docs project first

The marketing project needs the docs URL as an env var, so deploy docs
first.

1. **Vercel → New Project → Import Git Repository**, pick this repo.
2. **Root Directory**: `web/apps/docs`.
3. **Framework Preset**: Next.js (auto-detected).
4. **Build Command**: leave as default. Vercel runs the project's `build`
   script in the workspace root with `bun install` and `bun --filter
   @nativelink/docs build` automatically.
5. **Install Command**: `bun install` (auto-detected from `bun.lock`).
6. **Environment Variables**: none required.
7. Deploy. Note the assigned URL — e.g. `nativelink-docs.vercel.app`.

Verify: open `https://<docs-url>/docs` — should show the docs index.

### 2. Create the marketing project

1. **Vercel → New Project**, same repo.
2. **Root Directory**: `web/apps/web`.
3. **Framework Preset**: Next.js.
4. **Environment Variables** — add one:
   - `DOCS_URL` = `https://<docs-url-from-step-1>` (no trailing slash).
5. Deploy. The marketing app's `next.config.mjs` reads `DOCS_URL` and
   adds production rewrites:

   ```
   /docs        → ${DOCS_URL}/docs
   /docs/:path* → ${DOCS_URL}/docs/:path*
   ```

Verify: open `https://<marketing-url>/docs` — should serve the docs page
proxied from the docs deployment, including all assets (`/docs/_next/...`).

### 3. Custom domain

On the marketing project's **Settings → Domains**, add `nativelink.com`
(and `www.nativelink.com`). Point your DNS as Vercel instructs.

The docs project keeps its `*.vercel.app` URL — users never hit it
directly; they reach docs via the marketing rewrite.

## Local development

The marketing app's `next.config.mjs` rewrites `/docs/*` to the local
docs server in dev:

```bash
# from web/
bun dev                          # both apps via Turbo
# → http://localhost:3000/docs  (proxied to localhost:3001/docs)
```

`DOCS_DEV_URL` overrides the default `http://localhost:3001` target if
you run the docs server elsewhere.

## Environment variables

| Var               | Where         | Required | Default                  | Notes                                              |
| ----------------- | ------------- | :------: | ------------------------ | -------------------------------------------------- |
| `DOCS_URL`        | apps/web prod | ✓        | —                        | Full URL of the docs deployment, no trailing slash |
| `DOCS_DEV_URL`    | apps/web dev  |          | `http://localhost:3001`  | Override the local docs target                     |
| `GITHUB_TOKEN`    | apps/web      |          | unauthenticated          | Lifts the GitHub API rate limit on `/community`    |

## Preview deployments

Vercel creates a preview for every PR branch. The marketing preview's
`DOCS_URL` env var is set per-environment — by default it points at the
docs project's main-branch production. If you want a preview to proxy
to the docs project's branch preview, you'd need a build-time hook to
rewrite `DOCS_URL`; we haven't wired that yet.

## Generated content

The docs changelog page (`/reference/changelog`) is not checked in. It is
generated from the repository-root `CHANGELOG.md` — the canonical changelog
git-cliff maintains at release time — by
`apps/docs/scripts/gen-changelog.mjs`, which runs at the start of the docs
`dev` and `build` scripts. Every merge to main therefore republishes the
page from the current changelog, and the web CI build on pull requests
fails if a changelog edit stops compiling as MDX.

Because of this, don't configure an **Ignored Build Step** on the docs
Vercel project that skips builds when `web/` is unchanged: a release merge
touches only root files (`CHANGELOG.md`, version bumps), and skipping that
deploy would leave the published changelog stale until the next `web/`
change.

## Operating

- **Cache invalidation**: Vercel handles automatic edge invalidation on
  redeploy. No manual purge needed.
- **Build failures**: Vercel surfaces them in the project dashboard.
  Both apps `next build` cleanly today; common failures are dependency
  drift or env-var issues.
- **Rollback**: Vercel's **Deployments** tab → pick a green one →
  **Promote to Production**.

## Skipping the proxy

If you'd rather host docs on a subdomain (`docs.nativelink.com`) instead
of proxying through marketing:

1. Drop the rewrite block from `apps/web/next.config.mjs`.
2. Set `apps/docs/next.config.mjs` `basePath` to `""`.
3. Move the catch-all from `app/[[...slug]]` to whatever URL shape you
   want.
4. Point `docs.nativelink.com` at the docs Vercel project.

We chose the apex-with-rewrite approach because it keeps every page
under one origin (better for SEO, cookies, analytics, search-console
verification).
