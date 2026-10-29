import { CtaButton } from "@/components/cta-button";
import { CLONE_THE_REPO, GET_STARTED, SEE_PRICING, TALK_TO_US } from "@/lib/cta";
import { LLMS_FILES, llmsFileBytes, sizeLine } from "@/lib/llms-files";
import { AskAi, Badge, Button, Eyebrow, Reveal, Section, askAiPrompt } from "@nativelink/ui";

export const metadata = {
  title: "For agents",
  description:
    "What NativeLink is and how to read about it as a machine: the fetch order for the three llms files with their sizes, the MCP server's five tools, skills for Claude Code, Codex and Cursor, and the rules for contributing with an agent.",
};

const SITE = "https://nativelink.com";
const DOCS = "https://docs.nativelink.com";
const REPO = "https://github.com/TraceMachina/nativelink";
const CONTACT_EMAIL = "contact@tracemachina.com";
const BOOK_PATH = "/resources/oreilly-bazel-book";

/** The reading protocol, addressed to the agent. Mirrored in lib/llms.ts and AGENTS.md. */
const protocol = [
  {
    step: "Fetch /llms.txt first.",
    body: "It is the index: every page with a one-line description, in reading order. Decide from it what to fetch next.",
  },
  {
    step: "For most questions, fetch /llms-small.txt.",
    body: "Every page of this site in prose, plus the documentation's headings and the sentence that opens each section.",
  },
  {
    step: "Fetch /llms-full.txt only when the small file lacks the answer.",
    body: "It is the whole corpus with documentation bodies inlined, and it is large; the size is printed below.",
  },
  {
    step: "Cite documentation by URL.",
    body: "Every section of the full file is delimited by its canonical URL. Quote that URL, not this page.",
  },
  {
    step: "Never quote a price.",
    body: `This site prints none. Send people to ${CONTACT_EMAIL}; a person replies within a day.`,
  },
];

const mcpTools = [
  {
    name: "Generate a Bazel configuration",
    body: "A .bazelrc with remote cache and execution settings for the project in front of the agent.",
  },
  {
    name: "Fetch documentation and best practices",
    body: "The matching pages from docs.nativelink.com, so the agent's answer cites the source.",
  },
  {
    name: "Analyse build performance",
    body: "Where the time goes, the cache hit rate, and what to change.",
  },
  {
    name: "Watch files, build and test on change",
    body: "Rebuild and retest as files change, with results back in the editor.",
  },
  {
    name: "List recent builds",
    body: "The latest invocations from the Build Event Protocol, with status and timing.",
  },
];

const installs = [
  {
    agent: "Claude Code",
    dir: "~/.claude/skills/<name>",
    project: ".claude/skills/<name> in the repository",
    invoke: "/<name>",
  },
  {
    agent: "Codex",
    dir: "~/.agents/skills/<name>",
    project: ".agents/skills/<name>, anywhere from the working directory up to the repository root",
    invoke: "$<name>, or /skills to list them",
  },
  {
    agent: "Cursor",
    dir: "~/.cursor/skills/<name> or ~/.agents/skills/<name>",
    project:
      ".cursor/skills/<name> or .agents/skills/<name>; Cursor also reads the Claude Code and Codex directories above",
    invoke: "type / in Agent chat",
  },
];

const skills = [
  {
    name: "migrate-to-bazelmod",
    tag: "Migration",
    body: "Move a Bazel project from WORKSPACE to MODULE.bazel: the hybrid gradual path, dependency translation, toolchain registration, and smoke tests for library authors who have to support both.",
    href: `${REPO}/tree/main/.claude/skills/migrate-to-bazelmod`,
  },
  {
    name: "nativelink-*",
    tag: "Working on NativeLink",
    body: "Five playbooks the NativeLink team's own agents follow: Bazel verification, config and protocol changes, dependency updates, Local Remote Execution debugging, and Rust changes across the crates.",
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

const linkClass = "text-brand underline-offset-4 hover:underline";

export default function AgentsPage() {
  const sizes = Object.fromEntries(
    LLMS_FILES.map((f) => [f.name, sizeLine(llmsFileBytes(f.name))]),
  );

  return (
    <>
      {/* HERO */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[560px] bg-[radial-gradient(ellipse_1000px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.16),transparent_70%)]" />
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[500px] bg-dot-grid opacity-40 [mask-image:radial-gradient(ellipse_at_top,black_20%,transparent_70%)]" />
        <Section width="default" className="pt-24 pb-14 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[900px] text-center">
              <Eyebrow className="mb-5">For agents</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] md:text-[64px]">
                Built to be read by{" "}
                <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
                  agents as well as people
                </span>
                .
              </h1>
              <p className="mx-auto mt-6 max-w-[720px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                NativeLink is a remote build cache and remote execution server for the Remote
                Execution API, written in Rust and source available. Its website, documentation and
                repository are written for a machine reader too: nothing on this page needs an
                account, an API key or JavaScript.
              </p>
            </div>
          </Reveal>
        </Section>
      </section>

      {/* PROTOCOL: addressed to the agent */}
      <Section width="default" className="pb-20">
        <Reveal>
          <div className="mx-auto max-w-[900px] overflow-hidden rounded-2xl border border-border bg-surface">
            <div className="flex items-center gap-3 border-b border-border/60 bg-surface-elevated/60 px-6 py-3 font-mono text-[11px] uppercase tracking-[0.18em] text-muted">
              <span className="h-2 w-2 rounded-full bg-brand" aria-hidden="true" />
              If you are the agent reading this
            </div>
            <ol className="grid gap-px bg-border md:grid-cols-5">
              {protocol.map((p, i) => (
                <li key={p.step} className="flex flex-col gap-2 bg-surface p-5">
                  <span className="font-mono text-sm text-brand">0{i + 1}</span>
                  <p className="font-semibold leading-snug text-foreground">{p.step}</p>
                  <p className="text-sm leading-relaxed text-muted-foreground [overflow-wrap:anywhere]">
                    {p.body}
                  </p>
                </li>
              ))}
            </ol>
          </div>
        </Reveal>
      </Section>

      {/* ASK AI: addressed to the person */}
      <Section width="default" className="pb-20">
        <Reveal>
          <div className="mx-auto max-w-[900px] rounded-2xl border border-border bg-surface p-8 md:p-10">
            <p className="mb-6 text-center font-mono text-[11px] uppercase tracking-[0.18em] text-muted">
              If you are the person
            </p>
            <AskAi siteUrl={SITE} align="center" prompt={askAiPrompt(SITE, DOCS)} />
            <p className="mt-6 text-sm leading-relaxed text-muted-foreground">
              Each button opens a hosted assistant with a prompt that follows the protocol above: it
              reads the llms files first, so its answer starts from what this site actually says,
              then asks about your build and your team and works towards a configuration for them.
              The same block sits in the footer of every page here, and at the bottom of every
              documentation page with a prompt for that page.
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
              disagreement with each other. The sizes below are measured from the generated files
              when the site is built, and the token counts assume four bytes per token. The
              documentation site serves its own three, built from its sidebar, and the same three
              files are committed at the root of the repository so a clone carries its own corpus.
            </p>
          </div>
        </Reveal>
        <div className="grid gap-4 md:grid-cols-3">
          {LLMS_FILES.map((f, i) => (
            <Reveal key={f.name} delay={i * 0.06}>
              <article className="flex h-full flex-col rounded-2xl border border-border bg-surface p-7">
                <p className="font-mono text-[10px] uppercase tracking-[0.18em] text-muted">
                  {f.label}
                </p>
                <h3 className="mt-2 font-mono text-xl font-semibold text-foreground">{f.name}</h3>
                {sizes[f.name] && (
                  <p className="mt-1 font-mono text-[13px] text-brand">{sizes[f.name]}</p>
                )}
                <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">{f.body}</p>
                <p className="mt-3 flex-1 text-[15px] leading-relaxed text-foreground">{f.when}</p>
                <ul className="mt-6 flex flex-col gap-1 font-mono text-[13px]">
                  <li>
                    <a href={`/${f.name}`} className={linkClass}>
                      nativelink.com/{f.name}
                    </a>
                  </li>
                  <li>
                    <a href={`${DOCS}/${f.name}`} className={linkClass}>
                      docs.nativelink.com/{f.name}
                    </a>
                  </li>
                  <li>
                    <a
                      href={`${REPO}/blob/main/${f.name}`}
                      target="_blank"
                      rel="noreferrer"
                      className={linkClass}
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
              className={linkClass}
            >
              AGENTS.md
            </a>
            , the map of the code for an agent changing it, and{" "}
            <a href={`${DOCS}/agents`} className={linkClass}>
              docs.nativelink.com/agents
            </a>
            , how to read the documentation end to end, cite it, and verify a claim against the
            source.
          </p>
        </Reveal>
      </Section>

      {/* MCP SERVER */}
      <Section id="mcp" width="default" className="scroll-mt-20 border-t border-border/60 py-24">
        <div className="grid gap-12 lg:grid-cols-[1fr_1.3fr] lg:gap-16">
          <Reveal>
            <Eyebrow className="mb-4">The MCP server</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Five tools, inside your coding agent.
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted-foreground md:text-lg">
              The NativeLink MCP server gives Claude Code, Cursor and Codex five tools for
              configuring and tuning builds without leaving the editor. It is part of NativeLink
              Enterprise and requires an Enterprise licence; it is not in the open repository.
              Everything else on this page is free.
            </p>
            <div className="mt-8 flex flex-wrap gap-3">
              <CtaButton cta={TALK_TO_US} />
              <Button size="lg" variant="outline" asChild>
                <a href="/#mcp">See it run</a>
              </Button>
            </div>
          </Reveal>
          <Reveal delay={0.1}>
            <ol className="grid gap-px overflow-hidden rounded-2xl border border-border bg-border">
              {mcpTools.map((t, i) => (
                <li key={t.name} className="flex gap-5 bg-surface px-6 py-5">
                  <span className="font-mono text-sm text-brand">0{i + 1}</span>
                  <div>
                    <p className="font-semibold text-foreground">{t.name}</p>
                    <p className="mt-1 text-sm leading-relaxed text-muted-foreground">{t.body}</p>
                  </div>
                </li>
              ))}
            </ol>
          </Reveal>
        </div>
      </Section>

      {/* SKILLS */}
      <Section width="default" className="border-t border-border/60 bg-surface-elevated/40 py-24">
        <Reveal>
          <div className="mb-12 max-w-[680px]">
            <Eyebrow className="mb-4">Skills for coding agents</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Playbooks an agent can install.
            </h2>
            <p className="mt-4 text-[16px] leading-relaxed text-muted-foreground">
              Each skill is a directory with a <span className="font-mono">SKILL.md</span> in the
              format Claude Code, Codex and Cursor all read; the{" "}
              <span className="font-mono">name</span> in its frontmatter matches the folder name.
              Claude Code picks them up automatically inside the repository. For another project,
              copy or symlink the folder into your agent's skills directory.
            </p>
          </div>
        </Reveal>
        <Reveal>
          <pre className="mb-4 overflow-x-auto rounded-2xl border border-border bg-surface p-5 font-mono text-[13px] leading-relaxed text-foreground/90">
            {`git clone ${REPO}\n`}
            {
              "cp -r nativelink/.claude/skills/migrate-to-bazelmod ~/.claude/skills/   # Claude Code\n"
            }
            {
              "cp -r nativelink/.claude/skills/migrate-to-bazelmod ~/.agents/skills/   # Codex, and Cursor\n"
            }
          </pre>
        </Reveal>
        <Reveal delay={0.05}>
          <div className="mb-8 grid gap-px overflow-hidden rounded-2xl border border-border bg-border md:grid-cols-3">
            {installs.map((a) => (
              <div key={a.agent} className="bg-surface p-5 text-sm leading-relaxed">
                <p className="font-semibold text-foreground">{a.agent}</p>
                <dl className="mt-2 grid gap-1 text-muted-foreground">
                  <div>
                    <dt className="font-mono text-[10px] uppercase tracking-[0.18em] text-muted">
                      User
                    </dt>
                    <dd className="font-mono text-[13px]">{a.dir}</dd>
                  </div>
                  <div>
                    <dt className="font-mono text-[10px] uppercase tracking-[0.18em] text-muted">
                      Project
                    </dt>
                    <dd>{a.project}</dd>
                  </div>
                  <div>
                    <dt className="font-mono text-[10px] uppercase tracking-[0.18em] text-muted">
                      Invoke
                    </dt>
                    <dd className="font-mono text-[13px]">{a.invoke}</dd>
                  </div>
                </dl>
              </div>
            ))}
          </div>
        </Reveal>
        <div className="grid gap-4 md:grid-cols-2">
          {skills.map((s, i) => (
            <Reveal key={s.name} delay={i * 0.06}>
              <article className="flex h-full flex-col rounded-2xl border border-border bg-surface p-7 transition-colors hover:border-brand/40">
                <div className="flex items-center justify-between gap-3">
                  <h3 className="font-mono text-lg font-semibold text-foreground">{s.name}</h3>
                  <Badge variant="outline">{s.tag}</Badge>
                </div>
                <p className="mt-4 flex-1 text-[15px] leading-relaxed text-muted-foreground">
                  {s.body}
                </p>
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
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
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
              <p>
                The long version is a free O'Reilly e-book,{" "}
                <a href={BOOK_PATH} className={linkClass}>
                  Extending Bazel to Its Full Potential
                </a>
                : cloud-native build infrastructure, parallelization, remote execution and caching
                with NativeLink. It is a PDF, sent in exchange for a work email, so an agent should
                send the person to that page rather than look for a direct link.
              </p>
            </div>
            <div className="mt-8 flex flex-wrap gap-3">
              <CtaButton cta={GET_STARTED} />
              <CtaButton cta={CLONE_THE_REPO} variant="outline" />
            </div>
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
          <p className="mx-auto mt-5 max-w-[600px] text-base leading-relaxed text-muted-foreground md:text-lg">
            Open Source is free, with no limit on cores, machines or users. Enterprise is one
            licence for your whole company, priced by company size, and this site prints no number.
            Write to <span className="font-mono text-foreground">{CONTACT_EMAIL}</span> and a person
            replies within a day, or{" "}
            <a href="/pricing#trial" className={linkClass}>
              request a six-week trial
            </a>
            . An assistant asked about pricing should say exactly that.
          </p>
          <div className="mt-8 flex flex-wrap justify-center gap-3">
            <CtaButton cta={TALK_TO_US} />
            <CtaButton cta={SEE_PRICING} variant="outline" />
          </div>
        </Reveal>
      </Section>
    </>
  );
}
