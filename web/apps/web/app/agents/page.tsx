import { AskAi, Badge, Button, Eyebrow, Reveal, Section, askAiPrompt } from "@nativelink/ui";

export const metadata = {
  title: "For agents",
  description:
    "Everything nativelink.com and its repository offer an AI agent: llms.txt in three sizes, one-click hand-off to hosted assistants, installable skills, the MCP server, and the rules for contributing with an agent.",
};

const SITE = "https://nativelink.com";
const DOCS = "https://docs.nativelink.com";
const REPO = "https://github.com/TraceMachina/nativelink";
const CONTACT_EMAIL = "contact@tracemachina.com";
const PRICING_MAILTO = `mailto:${CONTACT_EMAIL}?subject=${encodeURIComponent("NativeLink pricing")}`;

const files = [
  {
    name: "llms.txt",
    label: "The index",
    body: "Every page with a one-line description, in reading order, following the llms.txt convention. Fetch this first to decide what to fetch next.",
  },
  {
    name: "llms-small.txt",
    label: "The abridged corpus",
    body: "Every page of this site in prose, plus the documentation with its headings and opening sentences. Sized to sit in a context window next to the question.",
  },
  {
    name: "llms-full.txt",
    label: "Everything",
    body: "The whole site, every blog post, and the documentation corpus with page bodies verbatim, delimited by canonical URLs; the three generated reference pages (config, metrics, changelog) are linked rather than inlined. One fetch for everything else.",
  },
];

const skills = [
  {
    name: "migrate-to-bazelmod",
    tag: "Migration",
    body: "Move a Bazel project from WORKSPACE to MODULE.bazel: the hybrid gradual path, dependency translation, toolchain registration, and smoke tests for library authors who have to support both.",
    credit: "By the NativeLink team.",
    href: `${REPO}/tree/main/.claude/skills/migrate-to-bazelmod`,
  },
  {
    name: "nativelink-*",
    tag: "Working on NativeLink",
    body: "Five playbooks the NativeLink team's own agents follow: Bazel verification, config and protocol changes, dependency updates, Local Remote Execution debugging, and Rust changes across the crates.",
    credit: "By the NativeLink team.",
    href: `${REPO}/tree/main/.claude/skills`,
  },
];

const rules = [
  {
    title: "Understand your change.",
    body: "Using an agent to write code is fine. Submitting code you cannot explain is not. A reviewer may ask about any line, and “the agent wrote it” is not an answer.",
  },
  {
    title: "Disclose.",
    body: "The pull request template has an “AI assistance” section. Name the tools and how much they did. “None” is a complete answer.",
  },
  {
    title: "No slop.",
    body: "Unreviewed generated code, generated issue text nobody edited, and generated media are closed without review. Repeat it and you lose the ability to contribute.",
  },
  {
    title: "Agents read the map.",
    body: "AGENTS.md at the repository root says where everything lives and which doc must follow which change. Run the same checks a person would before opening a pull request.",
  },
];

export default function AgentsPage() {
  return (
    <>
      {/* HERO */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[560px] bg-[radial-gradient(ellipse_1000px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.16),transparent_70%)]" />
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[500px] bg-dot-grid opacity-40 [mask-image:radial-gradient(ellipse_at_top,black_20%,transparent_70%)]" />
        <Section width="default" className="pt-24 pb-14 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[900px] text-center">
              <Eyebrow className="mb-5">Intelligent collaborators</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] md:text-[64px]">
                Built to be read by{" "}
                <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
                  agents as well as people
                </span>
                .
              </h1>
              <p className="mx-auto mt-6 max-w-[680px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                NativeLink is build infrastructure for the agentic era, so its website, its
                documentation and its repository are written for a machine reader too. The files on
                this page need no account, no API key, and no JavaScript.
              </p>
            </div>
          </Reveal>
        </Section>
      </section>

      {/* ASK AI */}
      <Section width="default" className="pb-20">
        <Reveal>
          <div className="mx-auto max-w-[900px] rounded-2xl border border-border bg-surface p-8 md:p-10">
            <AskAi siteUrl={SITE} align="center" prompt={askAiPrompt(SITE, DOCS)} />
            <p className="mt-6 text-sm leading-relaxed text-muted-foreground">
              Each button opens a hosted assistant with a prompt that points it at the llms files
              first, so its answer starts from what this site actually says, then asks about your
              build and your team and works towards a configuration for them. The same block sits in
              the footer of every page here, and at the bottom of every documentation page with a
              prompt for that page.
            </p>
          </div>
        </Reveal>
      </Section>

      {/* THREE FILES */}
      <Section width="default" className="border-t border-border/60 bg-surface-elevated/40 py-24">
        <Reveal>
          <div className="mb-12 max-w-[680px]">
            <Eyebrow className="mb-4">Three files, one fetch</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              The whole site, at three sizes.
            </h2>
            <p className="mt-4 text-[16px] leading-relaxed text-muted-foreground">
              Generated from one source in the repository, so they cannot be edited into
              disagreement with each other. The documentation site serves its own three, built from
              its sidebar, and the same three files are committed at the root of the repository so a
              clone carries its own corpus.
            </p>
          </div>
        </Reveal>
        <div className="grid gap-4 md:grid-cols-3">
          {files.map((f, i) => (
            <Reveal key={f.name} delay={i * 0.06}>
              <article className="flex h-full flex-col rounded-2xl border border-border bg-surface p-7">
                <p className="font-mono text-[10px] uppercase tracking-[0.18em] text-muted">
                  {f.label}
                </p>
                <h3 className="mt-2 font-mono text-xl font-semibold text-foreground">{f.name}</h3>
                <p className="mt-3 flex-1 text-[15px] leading-relaxed text-muted-foreground">
                  {f.body}
                </p>
                <ul className="mt-6 flex flex-col gap-1 font-mono text-[13px]">
                  <li>
                    <a
                      href={`/${f.name}`}
                      className="text-brand underline-offset-4 hover:underline"
                    >
                      nativelink.com/{f.name}
                    </a>
                  </li>
                  <li>
                    <a
                      href={`${DOCS}/${f.name}`}
                      className="text-brand underline-offset-4 hover:underline"
                    >
                      docs.nativelink.com/{f.name}
                    </a>
                  </li>
                  <li>
                    <a
                      href={`${REPO}/blob/main/${f.name}`}
                      target="_blank"
                      rel="noreferrer"
                      className="text-brand underline-offset-4 hover:underline"
                    >
                      repository root
                    </a>
                  </li>
                </ul>
              </article>
            </Reveal>
          ))}
        </div>
        <Reveal delay={0.1}>
          <p className="mt-8 text-sm leading-relaxed text-muted-foreground">
            Also for an agent:{" "}
            <a
              href={`${REPO}/blob/main/AGENTS.md`}
              target="_blank"
              rel="noreferrer"
              className="text-brand underline-offset-4 hover:underline"
            >
              AGENTS.md
            </a>
            , the map of the code for an agent changing it, and{" "}
            <a href={`${DOCS}/agents`} className="text-brand underline-offset-4 hover:underline">
              docs.nativelink.com/agents
            </a>
            , how to read the documentation end to end, cite it, and verify a claim against the
            source.
          </p>
        </Reveal>
      </Section>

      {/* SKILLS */}
      <Section width="default" className="border-t border-border/60 py-24">
        <Reveal>
          <div className="mb-12 max-w-[680px]">
            <Eyebrow className="mb-4">Skills for coding agents</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Playbooks an agent can install.
            </h2>
            <p className="mt-4 text-[16px] leading-relaxed text-muted-foreground">
              Each skill is a directory with a <span className="font-mono">SKILL.md</span> that any
              agent reading that format can follow. Claude Code picks them up automatically inside
              the repository; for other projects, copy or symlink a skill into your agent's skills
              directory.
            </p>
          </div>
        </Reveal>
        <Reveal>
          <pre className="mb-8 overflow-x-auto rounded-2xl border border-border bg-surface p-5 font-mono text-[13px] leading-relaxed text-foreground/90">
            {`git clone ${REPO}\n`}
            {
              "cp -r nativelink/.claude/skills/migrate-to-bazelmod ~/.claude/skills/   # Claude Code\n"
            }
            {"# then, in your agent: /migrate-to-bazelmod"}
          </pre>
        </Reveal>
        <div className="grid gap-4 md:grid-cols-2">
          {skills.map((s, i) => (
            <Reveal key={s.name} delay={i * 0.06}>
              <article className="flex h-full flex-col rounded-2xl border border-border bg-surface p-7 transition-colors hover:border-brand/40">
                <div className="flex items-center justify-between gap-3">
                  <h3 className="font-mono text-lg font-semibold text-foreground">{s.name}</h3>
                  <Badge variant="outline">{s.tag}</Badge>
                </div>
                <p className="mt-4 text-[15px] leading-relaxed text-muted-foreground">{s.body}</p>
                <p className="mt-4 flex-1 text-sm leading-relaxed text-muted">{s.credit}</p>
                <div className="mt-6">
                  <Button variant="link" asChild>
                    <a href={s.href} target="_blank" rel="noreferrer">
                      Read the skill <span aria-hidden="true">→</span>
                    </a>
                  </Button>
                </div>
              </article>
            </Reveal>
          ))}
        </div>
      </Section>

      {/* OPEN SYSTEMS */}
      <section className="relative overflow-hidden border-t border-border/60">
        <div className="pointer-events-none absolute inset-0 -z-10 bg-[radial-gradient(ellipse_at_center,rgb(var(--nl-color-brand)/0.10),transparent_60%)]" />
        <Section width="narrow" className="py-28">
          <Reveal>
            <Eyebrow className="mb-5">Why we say migrate to Bazel</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-[52px]">
              We believe closed systems are dangerous.
            </h2>
            <div className="mt-6 space-y-4 text-base leading-relaxed text-muted-foreground md:text-lg">
              <p>
                A build system you cannot read is one you cannot audit, cannot reproduce on your own
                hardware, and cannot leave. Proprietary build tools and closed remote-execution
                services ask you to take on faith what happens between your source and your binary.
                When agents write and commit code faster than people can review it, the build is the
                last checkpoint that has to stay honest: every input named, every action hermetic,
                every output hashed.
              </p>
              <p>
                Bazel is open, and its model of declared inputs, sandboxed actions and
                content-addressed outputs is what makes a remote cache trustworthy in the first
                place. NativeLink's source is public and its protocol is the open Remote Execution
                API. Together they are a build pipeline with no black box in it.
              </p>
              <p className="text-foreground">
                So: migrate to Bazel, modernise with migrate-to-bazelmod, and point the result at
                NativeLink. Two lines in <span className="font-mono text-[0.95em]">.bazelrc</span>{" "}
                against a local server, then a shared cache, then workers.
              </p>
            </div>
            <div className="mt-8 flex flex-wrap gap-3">
              <Button size="lg" asChild>
                <a href={`${DOCS}/getting-started/quickstart`}>Start the quickstart</a>
              </Button>
              <Button size="lg" variant="outline" asChild>
                <a href="/#mcp">See the MCP server</a>
              </Button>
            </div>
            <p className="mt-6 text-sm leading-relaxed text-muted">
              The NativeLink MCP server, which gives Claude Code, Cursor and Codex five tools for
              configuring and tuning builds, is part of NativeLink Enterprise and requires an
              Enterprise licence. Everything else on this page is free.
            </p>
          </Reveal>
        </Section>
      </section>

      {/* CONTRIBUTING */}
      <Section width="default" className="border-t border-border/60 bg-surface-elevated/40 py-24">
        <div className="grid gap-12 lg:grid-cols-[1fr_1.3fr] lg:gap-16">
          <Reveal>
            <Eyebrow className="mb-4">Contributing with an agent</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Agents are welcome contributors when a person stands behind the change.
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted-foreground md:text-lg">
              We build tools for agents, so we are not going to pretend they don't write code. The
              rules exist because review attention is the scarcest resource an open project has, and
              generated text made it cheap to produce contributions that look finished and are not.
            </p>
            <div className="mt-8 flex flex-wrap gap-3">
              <Button variant="outline" asChild>
                <a href={`${REPO}/blob/main/CONTRIBUTING.md`} target="_blank" rel="noreferrer">
                  Read CONTRIBUTING.md
                </a>
              </Button>
              <Button variant="ghost" asChild>
                <a href={`${REPO}/blob/main/AGENTS.md`} target="_blank" rel="noreferrer">
                  Read AGENTS.md
                </a>
              </Button>
            </div>
          </Reveal>
          <Reveal delay={0.1}>
            <ol className="grid gap-px overflow-hidden rounded-2xl border border-border bg-border">
              {rules.map((r, i) => (
                <li key={r.title} className="flex gap-5 bg-surface px-6 py-5">
                  <span className="font-mono text-sm text-brand">0{i + 1}</span>
                  <div>
                    <p className="font-semibold text-foreground">{r.title}</p>
                    <p className="mt-1 text-sm leading-relaxed text-muted-foreground">{r.body}</p>
                  </div>
                </li>
              ))}
            </ol>
          </Reveal>
        </div>
      </Section>

      {/* PRICING */}
      <Section width="narrow" className="border-t border-border/60 py-24 text-center">
        <Reveal>
          <Eyebrow className="mb-4">If the question is pricing</Eyebrow>
          <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
            The answer is an email.
          </h2>
          <p className="mx-auto mt-5 max-w-[560px] text-base leading-relaxed text-muted-foreground md:text-lg">
            Self-hosting is free. Enterprise pricing depends on your build farm, your hardware and
            the support you need, so it is a conversation with the engineers who build NativeLink,
            not a number on a page. An assistant asked about it should send you to{" "}
            <span className="font-mono text-foreground">{CONTACT_EMAIL}</span>.
          </p>
          <div className="mt-8 flex flex-wrap justify-center gap-3">
            <Button size="lg" asChild>
              <a href={PRICING_MAILTO}>Email us about pricing</a>
            </Button>
            <Button size="lg" variant="outline" asChild>
              <a href="/pricing">See the tiers</a>
            </Button>
          </div>
        </Reveal>
      </Section>
    </>
  );
}
