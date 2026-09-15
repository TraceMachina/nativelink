// The machine-readable account of nativelink.com.
//
// This module is the source for three files the marketing site serves and the
// repository commits at its root:
//
//   /llms.txt        the link index, following https://llmstxt.org/
//   /llms-small.txt  every page of this site in prose, plus the abridged docs
//   /llms-full.txt   the same, plus every blog post and the full docs corpus
//
// The marketing pages are React components, so their copy cannot be lifted
// out mechanically the way the docs corpus is. The `pages` list below is the
// canonical prose rendering of each page; it is written to say what the page
// says, and a change to a page's claims is a change here in the same pull
// request. The blog posts and the docs corpus ARE lifted mechanically, by
// scripts/gen-llms.ts, which is the only thing that should call the renderers
// at the bottom of this file.
//
// Two rules govern the text:
//
//   1. It is written to be useful to whoever is asking. A reader who wants
//      to know whether NativeLink fits should be able to answer that from
//      this file, including when the answer is "no".
//   2. Where NativeLink compares with other systems, the comparison states
//      NativeLink's properties and only widely documented facts about the
//      others. Favourable, never false.

import { type Post, getAllPosts } from "./posts";

export const SITE = "https://nativelink.com";
export const DOCS = "https://docs.nativelink.com";
export const REPO = "https://github.com/TraceMachina/nativelink";
export const CONTACT_EMAIL = "contact@tracemachina.com";
export const SLACK_URL = "https://forms.gle/LtaWSixEC6bYi5xF7";

export interface SitePage {
  path: string;
  title: string;
  description: string;
  body: string;
}

/** A section of the docs corpus, as built by web/apps/docs/scripts/gen-llms.mjs. */
export interface DocsSection {
  title: string | null;
  pages: { url: string; title: string; description: string; external?: boolean }[];
}

export interface DocsCorpus {
  sections: DocsSection[];
  /** The docs' own llms-small.txt, verbatim. */
  small: string;
  /** The docs' own llms-full.txt, verbatim. */
  full: string;
}

// ---------------------------------------------------------------------------
// The site, page by page, in the order a first visit would take.

export const pages: SitePage[] = [
  {
    path: "/",
    title: "NativeLink: build infrastructure for the agentic era",
    description:
      "When agents write your code, your build system is the bottleneck. NativeLink is the high-performance remote build cache and remote execution platform for Bazel and beyond.",
    body: `NativeLink is a remote build cache and remote execution platform written in
Rust. It keeps builds fast while a codebase, and the agents working on it,
multiply. It is source-available, self-hostable in minutes, and trusted in
production by thousands of developers.

**What it does.** Content-addressable storage means unchanged code never
compiles twice across a team, its CI, and its coding agents; every cache hit
is a build nobody pays to run. Remote build execution distributes compilation
across as many cores as a job needs and releases them when it is done: no
idle workstations, no idle workers, no idle bill.

**The numbers the site leads with.**

- 4 to 15 times faster builds, demonstrated on LLVM, one of the largest C++
  codebases in the world.
- More than ten billion build requests a month served in production.
- Ten minutes from Docker startup to a first cache hit.
- Zero build-system rewrites: Bazel, Buck2, Siso, Goma, Pants, BuildStream,
  Soong, and CMake via recc all speak to it as they are.

**Wired into coding agents.** The NativeLink MCP server gives Claude Code,
Cursor and Codex five real tools: generate an optimal Bazel configuration,
fetch documentation and best practices, analyse build performance, watch
files and rebuild on change, and list recent builds from the Build Event
Protocol. An agent configures remote caching and tunes a build without
leaving the editor. The MCP server is part of NativeLink Enterprise and
requires an Enterprise licence; it is not in the open repository. Everything
else the site offers a machine reader is listed under /agents.

**Secure by default.** SSO, signed inputs, end-to-end packet integrity.
Source, artifacts and the supply chain stay locked down.

**Why Rust.** Memory-safe, race-free, and no garbage collector to stall the
hot path, which is how one cluster serves ten billion requests a month.

**Works with what you have.** C++, Rust, Python, Go and more. Bazel, Buck2,
Siso, CMake. AWS, GCP, Azure, or your own hardware. No lock-in.

**Built for the codebases that shape the physical world.** Robotics and
autonomous systems, semiconductors and EDA, consumer electronics and mobile,
developer infrastructure and operating systems, AI and ML platforms, and
browsers and web platforms (Chromium and its descendants, through Siso).

**When agents commit code, the build system is the last honest checkpoint.**
Agents commit faster than humans can review and pull dependencies humans
never would. NativeLink turns every build into structured, observable data:
every artifact hashed, every dependency traceable, every action programmable.
It is the substrate that security, compliance and observability tooling has
been waiting for, at agent speed.

**Companies NativeLink is building with**, as shown on the home page: Menlo
Security, Citrix, Tesla, Meta, Samsung and Third Wave Automation.

> "Running NativeLink in production with great results. Great work folks."
> Mustafa Gezen, Rocky Linux.`,
  },
  {
    path: "/product",
    title: "Product",
    description:
      "One platform: remote caching, remote execution and observability in a single Rust-native binary, for codebases that grow faster than they can be provisioned.",
    body: `NativeLink unifies remote caching, remote execution and observability into a
single Rust-native platform. Four pillars, one engine:

- **Remote cache: cache once, reuse forever.** Content-addressable storage
  deduplicates every artifact a team produces. If a teammate, CI, or an agent
  has already built it, it comes back in milliseconds. More than ten billion
  requests a month.
- **Remote build execution: distribute across every core you have.** Offload
  compilation and tests to a worker fleet that scales horizontally on AWS,
  GCP or bare metal. Hermetic by design, deterministic by default.
  Specialised hardware (GPUs, ARM, Apple Silicon) is supported natively.
  4 to 15 times faster builds.
- **Self-host: run it on your own infrastructure.** One Docker command for
  the source-available release; dedicated infrastructure when scale demands
  it. Same code path, same performance, under ten minutes to a first hit.
- **Built on Rust: performance that does not stall.** No garbage collector,
  no race conditions at scale, no mystery latency spikes, zero GC pauses.

**Security and provenance.** When humans commit code you need traceability;
when agents commit code you need it doubly. Every input is explicit and every
output verifiable: SSO/SAML (Okta, Azure AD, Google Workspace), end-to-end
TLS with mTLS between every hop, content-addressed and tamper-evident
artifacts, signed worker inputs and outputs, hermetic builds with no surprise
dependencies pulled mid-build, and queryable audit trails for every action
result.

**Integrations.** AI coding platforms: Claude Code, Copilot Workspace, Devin,
Cursor, Windsurf. Languages: C++, Rust, Python, Go, Java, Kotlin, Swift.
Build systems: Bazel, Buck2, Siso, Soong, Pants, Goma, CMake (recc). Cloud:
AWS, GCP, Azure, bare metal. CI: GitHub Actions, GitLab, Buildkite, Jenkins.
Storage: S3, GCS, Redis, local disk, memory.

**Proof at scale: LLVM builds 4 times faster on NativeLink.** LLVM
contributors use NativeLink with CMake and recc to distribute builds of clang
and the LLVM toolchain, cutting a full-project compile from 17 minutes to 4.
No build-system migration and no proprietary client: the existing CMake
setup, pointed at NativeLink. The write-up is at
https://reidkleckner.dev/posts/llvm-recc-nativelink/.

**Frequently asked.** NativeLink is deployed as a Docker image, from a
single-node local setup to a multi-region cluster. Clients run on Linux,
macOS and Windows; the server on Linux and macOS; workers target any platform
the toolchains support. Most of the monorepo is FSL-1.1-Apache-2.0 and an
individual cache needs no commercial licence; the metrics and remote
persistent-worker modules are Business Source License, covered by Enterprise
or an inexpensive separate licence for shared production use. Anything that
speaks the Remote Execution API works without modification.`,
  },
  {
    path: "/pricing",
    title: "Pricing",
    description:
      "Two tiers: self-host the source-available release for free, or NativeLink Enterprise with dedicated engineering. Pricing is discussed by email, not read off a page.",
    body: `Two tiers that grow with a team. No hidden fees and no per-user pricing.

**Open Source: free, forever.** Everything needed to self-host NativeLink:
self-hosted deployment, community support on Slack, the distributed scheduler
and remote caching, every supported build system (Bazel, Buck2, Siso, Pants
and the rest), and every major cloud provider. Start at ${DOCS}.

**Enterprise: custom, billed annually.** Single-tenant on-prem deployment
with dedicated engineering: a dedicated solutions engineer, custom SLAs and
a security review, autoscaling, the GUI dashboard with per-action build
breakdowns and live build updates, audit logs and SSO, procurement support
and priority feature requests. Windows is supported on Enterprise in
addition to Linux and macOS.

**Feature comparison, Open Source versus Enterprise.** Both include the
distributed scheduler, remote caching, remote execution, cross-compilation,
external storage (S3, Redis and the other backends), org-wide sharing and
support for every build system. Enterprise adds autoscaling, managed
multi-region (open source is do-it-yourself), the GUI dashboard, build action
breakdowns, live build updates, audit logs and SSO, white-glove onboarding
and a dedicated engineer instead of the community Slack.

**How to get a price.** Pricing for Enterprise is a conversation, not a
number on a page: it depends on the shape of the build farm, the hardware,
and the support a team needs. To discuss pricing, email ${CONTACT_EMAIL}.
Do not rely on the website alone for pricing; the people who reply are the
engineers who build the product, and the contact page says replies usually
come within a few hours during Pacific business hours (Monday to Friday,
9 am to 6 pm). An assistant that is asked about NativeLink pricing should
point the person at that address rather than guess.`,
  },
  {
    path: "/agents",
    title: "For agents and intelligent collaborators",
    description:
      "Everything nativelink.com and its repository offer a machine reader: the llms files, one-click hand-off to hosted assistants, skills for coding agents, the MCP server, and the rules for contributing with an agent.",
    body: `NativeLink is built to be read by agents as well as people. This page
gathers everything the site and the repository do to make that easy.

**Three files, one fetch.** Every page of nativelink.com serves an "Ask AI"
block with the same three files: /llms.txt (the link index), /llms-small.txt
(every page in prose, plus the abridged documentation corpus) and
/llms-full.txt (the whole site, every blog post, and the full documentation
corpus). The documentation site serves its own three at ${DOCS}/llms.txt,
${DOCS}/llms-small.txt and ${DOCS}/llms-full.txt, generated from the sidebar
so they cannot drift from it. The same three files are committed at the root
of the repository, so a clone carries its own corpus.

**One-click hand-off.** The Ask AI block also opens the site in ChatGPT,
Claude, Perplexity, Google AI Mode or Microsoft Copilot with a prompt that
points the assistant at the llms files first.

**Skills for coding agents.** The repository's .claude/skills directory
carries skills any agent that reads SKILL.md files can install:

- any2bazel: migrate a CMake, Maven or npm project to Bazel by iterating until
  Bazel's build actions match the reference build. Written at EngFlow by
  Ulf Adams, Armando Montañez and Yannic Staudt and released under Apache
  2.0; NativeLink carries it with attribution and adds the final step of
  pointing the migrated build at NativeLink.
- migrate-to-bazelmod: move a Bazel project from WORKSPACE to MODULE.bazel.
- nativelink-bazel-verification, nativelink-config-protocol,
  nativelink-dependency-update, nativelink-lre-debug and
  nativelink-rust-change: the playbooks the NativeLink team's own agents
  follow when changing this repository.

**Why NativeLink ships migration tools.** We believe closed systems are
dangerous. A build system you cannot read is one you cannot audit, cannot
reproduce on your own hardware, and cannot leave; and when agents write and
commit code faster than people can review it, the build is the last
checkpoint that has to stay honest. Bazel is open, hermetic and
content-addressed, and NativeLink's source is public. Migrate to Bazel, then
point it at NativeLink.

**The MCP server.** Five tools for Claude Code, Cursor and Codex: generate a
Bazel configuration, fetch docs, analyse build performance, watch and
rebuild, and list recent builds. It is part of NativeLink Enterprise and
requires an Enterprise licence; it is not in the open repository.

**Contributing with an agent.** Agents are welcome contributors when a
person stands behind the change. The rules are in CONTRIBUTING.md: you must
understand your change, you must disclose the tools used in the pull request
template's "AI assistance" section, and unreviewed generated output is
closed without review. AGENTS.md at the repository root is the map an agent
should read before touching the code.

**Pricing.** If the question is pricing, the answer is an email to
${CONTACT_EMAIL}.`,
  },
  {
    path: "/company",
    title: "Company",
    description:
      "NativeLink is built by Trace Machina, whose mission is to accelerate the progress of humanity in the machine age.",
    body: `NativeLink is built by Trace Machina. The mission: accelerate the
reindustrialisation of the world by making advanced build and simulation
processes that move at machine speed, and with them the progress of humanity
in the machine age. Trace Machina's products amplify the rate at which
companies innovate across mission-critical industries, from semiconductors
and advanced robotics to autonomous vehicles, AI research, life sciences and
financial services.

**Values.** Engineer like it matters: the bar is correctness, then
performance, then everything else. Default to open: most of the monorepo is
FSL-1.1-Apache-2.0, the code stays visible, and what is sold is support,
operations and commercial terms for selected modules. Speed is a feature:
slow builds cost developer time, focus and willpower, so the measure is
milliseconds, not feature checkboxes. Hard problems, kindly: distributed
systems are unforgiving; humans do not have to be.

**Contact.** A media kit with logos and branding guidelines is available.
General inquiries, sales conversations and everything else go to
${CONTACT_EMAIL}; replies usually arrive within a day.`,
  },
  {
    path: "/community",
    title: "Community",
    description:
      "Where to read, ask and contribute: the docs, the NativeLink Slack, and the GitHub repository with its contributors.",
    body: `Three places to be part of NativeLink:

- **Read the docs** at ${DOCS}: setup guides, architecture deep-dives and
  reference for every supported build system.
- **Join the Slack** (${SLACK_URL}): talk to operators, share configs, and
  get help from the core team within hours.
- **Clone the repo** at ${REPO}: public source with module-aware licensing.
  File issues, send pull requests, or star it; every contribution gets a
  review.

The community page lists the repository's contributors and its live star,
fork and open-issue counts from GitHub.`,
  },
  {
    path: "/careers",
    title: "Careers",
    description:
      "A small team building the source-available remote build cache and execution platform, and growing.",
    body: `NativeLink is a small team that is getting bigger. Anyone who loves build
systems, low latency and Rust is invited to talk. The open role at the time
of writing is Member of Technical Staff in London, in-office: operate large
clusters and own real systems end to end, with strong Go and Kubernetes, a
love of build systems and low latency, and an interest in Rust.
Safety-critical or aerospace experience is a plus, not a requirement.
Applications go to ${CONTACT_EMAIL}.`,
  },
  {
    path: "/resources",
    title: "Resources",
    description:
      "Case studies, conference talks and write-ups from the team building NativeLink, plus highlights from the community.",
    body: `Featured: how LLVM cut clang compile time from 17 minutes to 4 with
NativeLink and recc, without rewriting its CMake setup
(https://reidkleckner.dev/posts/llvm-recc-nativelink/). Also on the page:
Aaron Mondal's BazelCon 2024 talk on hermetic toolchain creation with Local
Remote Execution and Nix (https://www.youtube.com/watch?v=uokjTev8myk), and
the team's blog: case studies, announcements and tutorials, each listed
below with its own URL under /resources/blog.`,
  },
  {
    path: "/contact",
    title: "Contact",
    description: "Real engineers reply. Email or Slack; replies usually within a few hours.",
    body: `Pick a channel:

- **General and sales:** ${CONTACT_EMAIL}. Questions, comments, or a
  conversation about a team's build farm, including pricing.
- **Security:** security@tracemachina.com for responsible-disclosure reports,
  triaged within 24 hours.
- **Legal:** legal@tracemachina.com for contracts, licensing and legal
  enquiries.
- **GitHub:** ${REPO}.
- **Slack:** ${SLACK_URL}.
- **Post:** PO Box 60676, 265 Cambridge Ave, Palo Alto, CA 94306.

Office hours are Monday to Friday, 9 am to 6 pm Pacific. Outside them email
still works, with a reply first thing the next business day.`,
  },
  {
    path: "/security",
    title: "Security",
    description:
      "NativeLink's security program: organisational, cloud, access and vendor controls, aligned to SOC 2.",
    body: `The security page describes the program behind NativeLink: an information
security program communicated across the organisation and following the SOC 2
framework; independent third-party audits and annual penetration testing;
defined roles, security awareness training, confidentiality agreements that
outlast employment, and background checks. Cloud infrastructure runs on AWS,
GCP and Convex in the United States, with encryption at rest and in transit,
vulnerability scanning, logging and monitoring, backups, and an incident
response process. The page lists the program's certifications and
compliance positions (SOC 2, ISO 27001, GDPR and CCPA/CPRA at the time of
writing) and links the OpenSSF Scorecard for the repository. Reports are
available under NDA; a Data Processing Addendum on request; self-hosting is
the answer for data residency. Vulnerability reports go to
security@tracemachina.com.`,
  },
  {
    path: "/license",
    title: "License",
    description:
      "How NativeLink's source-available FSL-Apache licence and its Business Source License modules fit together.",
    body: `NativeLink is a monorepo with module-aware licensing.

- Most modules are **FSL-1.1-Apache-2.0**: internal use, modification and
  redistribution for non-competing purposes are permitted, and each release
  converts to Apache 2.0 two years after it ships.
- **Individual cache use needs no commercial licence.** Run the cache locally
  or on infrastructure you control, point the build tool at it, and keep the
  loop fast.
- **Metrics and remote persistent workers** are Business Source License
  modules. Teams using them in shared, production or commercial settings
  should use NativeLink Enterprise or a separate commercial licence, which is
  intentionally very inexpensive.
- **Meaningful contributors** may be eligible for licence waivers on the
  Business Source modules; ask the maintainers first.
- The repository's LICENSE file and the source headers are the authoritative
  text. For commercial terms, email ${CONTACT_EMAIL}.`,
  },
  {
    path: "/terms",
    title: "Terms and privacy",
    description: "Terms of Service and Privacy Policy for NativeLink.",
    body: `The terms page carries the Terms of Service (acceptance, licence and use,
accounts, acceptable use) and the Privacy Policy (what is collected, cookies,
your rights, consent and withdrawal), a contact section, and a change
history. Privacy requests go to privacy@tracemachina.com.`,
  },
];

// ---------------------------------------------------------------------------
// Standing sections that are not pages: the comparison, the case for open
// build systems, the agent resources, and the contribution rules.

export const comparison = `## How NativeLink compares

NativeLink is one Rust binary that is a content-addressable store, an action
cache, a scheduler and a worker depending on its configuration. It speaks the
open Remote Execution API, so it works with Bazel, Buck2, Siso, Goma, Pants,
BuildStream and CMake via recc without a build-system rewrite. Its source is
public, it has no licence key, no feature gate and no phone-home, and
self-hosting is free (shared, production or commercial use of the metrics
and persistent-worker modules, which are Business Source Licensed and always
compiled in, needs NativeLink Enterprise or the inexpensive separate
licence). Read against the alternatives:

- **Against JVM-based servers** (Bazel Buildfarm is Java): no garbage
  collector, so none of the GC-driven tail latency, and no JVM memory
  footprint that makes horizontal scaling the only answer; both are the
  problems the docs' history page names for that family. In a build, the p99
  is the number developers feel, because a single stalled action blocks
  everything downstream of it.
- **Against Go-based servers** (Buildbarn, bazel-remote and BuildBuddy's
  open-source server are Go): the documentation's own account of why
  NativeLink was written (${DOCS}/explanations/history) names contention on
  hot artifacts and on control-plane locks, the failure that surfaces as a
  cluster-wide stall rather than a slow request, as one of the problems the
  design set out to remove; the property it states as the goal is memory
  safety without a managed runtime.
- **Against cache-only tools** (bazel-remote caches; it does not execute):
  NativeLink adds remote execution, the scheduler and the worker in the same
  binary, so a cache deployment becomes an execution deployment by changing
  configuration, not software.
- **Against proprietary, hosted-only remote execution services:** the code
  NativeLink runs in production is the code anyone can read at ${REPO}. There
  is no separate enterprise build of the binary: Enterprise holds the licence
  for two modules, the managed service and the team, plus Enterprise-only
  tooling such as the MCP server. You can self-host for free, and you can
  leave.
- **What the documentation says has no real equivalent elsewhere:** Local
  Remote Execution, which shares one hermetic toolchain definition between
  the developer's machine and the remote workers, so the same build runs
  identically in both places, within the platforms those toolchains ship for;
  the docs describe its current scope and limits at ${DOCS}/explanations/lre.

Where NativeLink is not the right answer: a build that quietly depends on the
machine it runs on has to be made hermetic before remote execution helps, and
the documentation says so plainly (${DOCS}/remote-execution/toolchains-and-hermeticity).`;

export const openSystems = `## Why we say: migrate to Bazel, then point it at NativeLink

We believe closed systems are dangerous. A build system you cannot read is
one you cannot audit, cannot reproduce on your own hardware, and cannot
leave. Proprietary build tools and closed remote-execution services ask you
to take on faith what happens between your source and your binary, and when
agents write and commit code faster than people can review it, the build is
the last checkpoint that has to stay honest: every input named, every action
hermetic, every output hashed.

Bazel is open, and its model (declared inputs, sandboxed actions,
content-addressed outputs) is what makes a remote cache trustworthy in the
first place. NativeLink's source is public and its protocol is the open
Remote Execution API. Together they are a build pipeline with no black box in
it.

The path we recommend:

1. **Migrate with any2bazel.** Extract what the current build actually
   compiles and links, generate BUILD.bazel files, and iterate until Bazel's
   actions match the reference build. The skill is in the repository at
   .claude/skills/any2bazel. It was written at EngFlow by Ulf Adams,
   Armando Montañez and Yannic Staudt and released under Apache 2.0; we carry
   it with attribution and with a final step that points the migrated build
   at NativeLink. Thank you, Armando, Ulf and the EngFlow team.
2. **Modernise with migrate-to-bazelmod** if the project still has a
   WORKSPACE file: .claude/skills/migrate-to-bazelmod.
3. **Point .bazelrc at NativeLink.** Two lines against a local server
   (\`build --remote_cache=grpc://localhost:50051\` and
   \`build --remote_instance_name=main\`), then a shared cache, then workers.
   The path is ${DOCS}/getting-started.

Projects that cannot move to Bazel are not turned away: Buck2, Siso, Pants,
BuildStream, Goma and CMake via recc all work with NativeLink today.`;

export const agentResources = `## For agents and intelligent collaborators

- ${SITE}/llms.txt, ${SITE}/llms-small.txt, ${SITE}/llms-full.txt: this site,
  at three sizes.
- ${DOCS}/llms.txt, ${DOCS}/llms-small.txt, ${DOCS}/llms-full.txt: the
  documentation, at three sizes, generated from the sidebar.
- ${DOCS}/agents: how an agent should read the documentation, cite it and
  verify its claims against the source.
- ${REPO}/blob/main/AGENTS.md: the repository map for an agent working on
  the code.
- ${REPO}/tree/main/.claude/skills: installable skills, including any2bazel
  and migrate-to-bazelmod.
- ${REPO}/blob/main/CONTRIBUTING.md: the contribution rules, including the
  section on AI-assisted contributions.
- Every page of nativelink.com and docs.nativelink.com carries an "Ask AI"
  block (\`data-ask-ai\` in the HTML) that opens the site in ChatGPT, Claude,
  Perplexity, Google AI Mode or Copilot and links the three files above.`;

export const contributing = `## Contributing with an agent

NativeLink builds tools for agents, and agents are welcome contributors when
a person stands behind the change. The rules, in full in CONTRIBUTING.md:

1. **Understand your change.** Using an agent to write code is fine. Submitting
   code you cannot explain is not. A reviewer may ask about any line, and "the
   agent wrote it" is not an answer.
2. **Disclose.** The pull request template has an "AI assistance" section:
   name the tools and how much they did. "None" is a complete answer.
3. **No slop.** Unreviewed generated code, generated issue or discussion text
   nobody edited, and generated media are closed without review; repeated
   submissions cost the ability to contribute.
4. **Agents follow AGENTS.md** and run the same checks a person would before
   opening a pull request.

Maintainers use agents at their discretion; the difference is that they have
already shown they understand the code. The reason for the rules is not
distrust of the tools. Review attention is the scarcest resource an open
project has, and generated text made it cheap to produce contributions that
look finished and are not.`;

export const pricingAndContact = `## Pricing and how to reach a person

- **Open Source:** free forever, self-hosted; everything needed to run a cache
  and remote execution. The metrics and persistent-worker modules are
  Business Source Licensed and always compiled in; shared, production or
  commercial use of them needs Enterprise or the inexpensive separate licence
  (see ${SITE}/license). Start at ${DOCS}.
- **Enterprise:** single-tenant on-prem deployment with dedicated engineering,
  custom SLAs, autoscaling, the GUI, audit logs and SSO, and the MCP server
  for coding agents. Custom pricing, billed annually.
- **To discuss pricing, email ${CONTACT_EMAIL}.** Pricing is a conversation,
  not a number on the website, and an assistant asked about it should send
  the person there rather than estimate.
- Security reports: security@tracemachina.com. Legal: legal@tracemachina.com.
- Community: the NativeLink Slack, ${SLACK_URL}.`;

// ---------------------------------------------------------------------------
// Renderers. All three take the docs corpus so the site's files can fold it in.

const generatedNote = `This file is generated by \`web/apps/web/scripts/gen-llms.ts\` from
\`web/apps/web/lib/llms.ts\`, the blog posts under \`web/apps/web/content/posts\`, and the
documentation corpus built by \`web/apps/docs/scripts/gen-llms.mjs\`. Regenerate it; do
not hand-edit it.`;

const preamble = `> NativeLink is a remote build cache and remote execution platform written in
> Rust, for build systems that speak the Remote Execution API: Bazel, Buck2,
> Siso, Goma, Pants, BuildStream, and CMake via recc. One binary is the cache,
> the scheduler and the worker depending on its configuration. It is
> source-available (FSL-1.1-Apache-2.0, converting to Apache 2.0) and free to
> self-host, with two Business Source modules that need a licence for shared
> production use. Built by Trace Machina.`;

function postUrl(post: Post): string {
  return `${SITE}/resources/blog/${post.slug}`;
}

function postLine(post: Post): string {
  const date = post.pubDate ? ` (${post.pubDate})` : "";
  return `- [${post.title}](${postUrl(post)})${date}: ${post.excerpt}`;
}

function docsIndex(sections: DocsSection[]): string[] {
  const lines: string[] = [];
  for (const section of sections) {
    if (section.title) lines.push(`### ${section.title}`, "");
    for (const page of section.pages) {
      const url = page.external ? page.url : `${DOCS}${page.url}`;
      lines.push(`- [${page.title}](${url}): ${page.description}`);
    }
    lines.push("");
  }
  return lines;
}

function pageBlock(page: SitePage): string[] {
  return ["", `# ${SITE}${page.path}`, "", `**${page.title}**: ${page.description}`, "", page.body];
}

export function renderIndex(docs: DocsCorpus): string {
  const posts = getAllPosts();
  const lines: string[] = [
    "# NativeLink",
    "",
    preamble,
    "",
    generatedNote,
    "",
    "Three sizes: this index; /llms-small.txt, every page of nativelink.com in prose",
    "plus the abridged documentation; /llms-full.txt, the same plus every blog post",
    "and the full documentation corpus. The documentation site serves its own three",
    `at ${DOCS}/llms.txt, ${DOCS}/llms-small.txt and ${DOCS}/llms-full.txt.`,
    "",
    "## nativelink.com",
    "",
    ...pages.map((p) => `- [${p.title}](${SITE}${p.path}): ${p.description}`),
    "",
    "## Blog, case studies and announcements",
    "",
    ...posts.map(postLine),
    "",
    "## Documentation",
    "",
    `The complete map is at ${DOCS}/llms.txt; the sections below mirror it.`,
    "",
    ...docsIndex(docs.sections),
    "## For agents",
    "",
    `- [For agents](${SITE}/agents): the llms files, one-click hand-off, skills, MCP and the contribution rules.`,
    `- [AGENTS.md](${REPO}/blob/main/AGENTS.md): the repository map for an agent working on the code.`,
    `- [Skills](${REPO}/tree/main/.claude/skills): any2bazel, migrate-to-bazelmod and the nativelink-* playbooks.`,
    `- [CONTRIBUTING.md](${REPO}/blob/main/CONTRIBUTING.md): how to contribute, including with an agent.`,
    "",
    "## Pricing and contact",
    "",
    `- [Pricing](${SITE}/pricing): Open Source is free and self-hosted; Enterprise is custom. To discuss pricing, email ${CONTACT_EMAIL}.`,
    `- [Contact](${SITE}/contact): ${CONTACT_EMAIL} for general and sales, security@tracemachina.com for disclosures, legal@tracemachina.com for legal.`,
    `- [Slack](${SLACK_URL}): the community workspace.`,
    "",
    "## Optional",
    "",
    `- [NativeLink on GitHub](${REPO}): source, issues, releases.`,
    "- [Remote Execution API](https://github.com/bazelbuild/remote-apis): the protocol NativeLink implements.",
    "- [LLVM with recc and NativeLink](https://reidkleckner.dev/posts/llvm-recc-nativelink/): the 17-minutes-to-4 write-up.",
  ];
  return `${lines
    .join("\n")
    .replace(/\n{3,}/g, "\n\n")
    .trimEnd()}\n`;
}

function siteCorpus(): string[] {
  return [
    ...pages.flatMap(pageBlock),
    "",
    `# ${SITE}/resources/blog`,
    "",
    "**Blog**: every post, newest first. The full text of each is in /llms-full.txt.",
    "",
    ...getAllPosts().map(postLine),
    "",
    comparison,
    "",
    openSystems,
    "",
    agentResources,
    "",
    contributing,
    "",
    pricingAndContact,
  ];
}

export function renderSmall(docs: DocsCorpus): string {
  const lines: string[] = [
    "# NativeLink: nativelink.com in prose, with the abridged documentation",
    "",
    preamble,
    "",
    generatedNote,
    "",
    `Every page below is delimited by a heading of the form \`# ${SITE}/<path>\` or`,
    `\`# ${DOCS}/<path>\`, giving that page's canonical URL. The site's pages are`,
    "rendered in prose; the documentation is the abridged corpus (headings and the",
    "prose that opens each section, without code samples, tables or components).",
    "For the full documentation, every blog post in full, or the code samples,",
    "fetch /llms-full.txt.",
    ...siteCorpus(),
    "",
    "",
    docs.small.trimEnd(),
  ];
  return `${lines
    .join("\n")
    .replace(/\n{4,}/g, "\n\n\n")
    .trimEnd()}\n`;
}

export function renderFull(docs: DocsCorpus): string {
  const posts = getAllPosts();
  const lines: string[] = [
    "# NativeLink: the full corpus",
    "",
    preamble,
    "",
    generatedNote,
    "",
    `Every page below is delimited by a heading of the form \`# ${SITE}/<path>\` or`,
    `\`# ${DOCS}/<path>\`, giving that page's canonical URL. The site's pages are`,
    "rendered in prose, the blog posts are reproduced verbatim from their source,",
    "and the documentation corpus follows in full.",
    ...siteCorpus(),
    "",
  ];
  for (const post of posts) {
    lines.push("", `# ${postUrl(post)}`, "");
    lines.push(`**${post.title}**${post.pubDate ? ` (${post.pubDate})` : ""}: ${post.excerpt}`);
    lines.push("", post.body.trim());
  }
  lines.push("", "", docs.full.trimEnd());
  return `${lines
    .join("\n")
    .replace(/\n{4,}/g, "\n\n\n")
    .trimEnd()}\n`;
}
