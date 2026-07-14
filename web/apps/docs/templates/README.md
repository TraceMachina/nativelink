# Page templates

Four archetypes, one file each. Copy the one that matches what you're writing
into `content/docs/`, keep the structure, replace the content.

These files live outside `content/docs/` on purpose: `source.config.ts`
publishes everything under that directory, and a template is not a page.

| Archetype | Use it when the reader wants to | File |
|---|---|---|
| Tutorial | learn by doing, on a guaranteed happy path | `tutorial.mdx` |
| How-to | accomplish one specific task they already have | `how-to.mdx` |
| Explanation | understand why something is shaped the way it is | `explanation.mdx` |
| Reference | look up an exact fact | `reference.mdx` |

Three rules cut across all four.

**Reading path vs lookup surface.** Narrative pages never inline an
exhaustive field list. They explain the fields that carry a decision and link
the generated reference for the rest. The reference is exhaustive so the
narrative doesn't have to be.

**Layer 1 → 2 → 3.** Layer 1 orients and shows the whole thing. Layer 2 is
the working detail. Layer 3 is the edge cases, the troubleshooting, and the
handoff. A beginner stops after layer 1 and is not lost; an expert skips to
layer 2 and is not patronised.

**Every page in an ordered track states its rung.** Use `<Rung>`. A reader
who lands cold from a search result should be told what the page assumes and
sent back one rung if they don't have it.
