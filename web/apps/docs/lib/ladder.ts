/**
 * The CAS-to-RBE ladder.
 *
 * The docs are ordered as a climb rather than a catalogue: get a cache serving
 * hits, then put workers behind the misses, then own the config file, then
 * reach for recipes. This array is the single definition of that order. The
 * sidebar dividers, the `<Rung>` badge on each page, and the generated
 * `llms.txt` all read from it, so an agent traversing the docs by machine and a
 * human traversing them by sidebar climb the same ladder in the same order.
 */
export interface LadderRung {
  /** 1-indexed position on the ladder. */
  n: number;
  /** Sidebar label for this rung. */
  title: string;
  /** The group landing page. */
  href: string;
  /** What the reader has when they finish it. */
  outcome: string;
}

export const LADDER: readonly LadderRung[] = [
  {
    n: 1,
    title: "Start with caching",
    href: "/getting-started",
    outcome: "a cache serving hits to your build",
  },
  {
    n: 2,
    title: "Add remote execution",
    href: "/remote-execution",
    outcome: "workers running the actions that miss the cache",
  },
  {
    n: 3,
    title: "Configure it",
    href: "/configuration",
    outcome: "a config file you wrote and understand",
  },
  {
    n: 4,
    title: "Task recipes",
    href: "/how-to",
    outcome: "the backend, transport, and upgrade path you actually want",
  },
];

export const LADDER_LENGTH = LADDER.length;

export function rungAt(n: number): LadderRung | undefined {
  return LADDER.find((rung) => rung.n === n);
}
