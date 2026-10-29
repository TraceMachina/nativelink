/**
 * Claims that appear on more than one page, written once.
 *
 * The home page stats, the Product pillars and the LLVM proof row all read
 * from here, so "0 build-system rewrites required" on one page cannot become
 * "0 build-system rewrite" on another. Change a claim here and it changes
 * everywhere, in the same words.
 */
export const CLAIMS = {
  fasterBuilds: {
    value: "4-15×",
    label: "faster builds",
    body: "Demonstrated on LLVM, one of the world's largest C++ codebases.",
  },
  requestsPerMonth: {
    value: "10B+",
    label: "build requests per month",
    body: "Served in production.",
  },
  timeToFirstHit: {
    value: "10m",
    label: "to your first cache hit",
    body: "From Docker startup to a reusable remote cache.",
  },
  // The noun lives in the value on purpose: a bare "0" among big numbers reads as a failure.
  noRewrites: {
    value: "0 rewrites",
    label: "of your build system",
    body: "Bazel, Buck2, Siso, Goma, Pants, and CMake via recc, as they are.",
  },
} as const;

/** The LLVM result the Product page leads its proof section with. */
export const LLVM = {
  speedup: { value: "4×", label: "faster LLVM builds" },
  wallTime: { value: "17m → 4m", label: "full-project compile" },
} as const;
