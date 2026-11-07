/**
 * Claims that appear on more than one page, written once, so no page can
 * shorten or reword them differently.
 */

/** Every build system NativeLink speaks, in one list, so no page can shorten it differently. */
export const BUILD_SYSTEMS = [
  "Bazel",
  "Buck2",
  "Siso",
  "Pants",
  "Goma",
  "Soong",
  "CMake via recc",
  "BuildStream",
  "GN",
  "Cargo",
] as const;

export const BUILD_SYSTEMS_SENTENCE = BUILD_SYSTEMS.join(", ");
