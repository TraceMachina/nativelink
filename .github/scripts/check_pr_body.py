#!/usr/bin/env python3
"""Check that a pull request body has the template's sections filled in.

Shape only. Whether a section is empty is something a script can know;
whether it is honest is not, so nothing here tries to judge the content.

Usage: check_pr_body.py <path-to-body-file>

Exits 0 when the body is usable, 1 otherwise, printing what is missing in
markdown suitable for posting as a comment.
"""

import re
import sys

# Each section a reviewer cannot reconstruct from the diff or from CI.
REQUIRED_SECTIONS = ("What and why", "How this was verified", "Risk")

# Long enough to rule out "n/a" and "see title", short enough that one real
# sentence clears it.
MIN_SECTION_CHARS = 40

HTML_COMMENT = re.compile(r"<!--.*?-->", re.DOTALL)


def section_body(body: str, heading: str) -> str | None:
    """Returns the text under `heading`, or None if the heading is absent."""
    pattern = re.compile(
        rf"^\#{{1,6}}\s*{re.escape(heading)}\s*$(.*?)(?=^\#{{1,6}}\s|\Z)",
        re.DOTALL | re.MULTILINE | re.IGNORECASE,
    )
    match = pattern.search(body)
    if match is None:
        return None
    # Comments are invisible to a reader, so they do not count as content.
    return HTML_COMMENT.sub("", match.group(1)).strip()


def check(body: str) -> list[str]:
    if not body.strip():
        return ["The description is empty. Please fill in the template."]

    problems = []
    for heading in REQUIRED_SECTIONS:
        content = section_body(body, heading)
        if content is None:
            problems.append(
                f"**{heading}** is missing. Please keep the template's headings."
            )
        elif len(content) < MIN_SECTION_CHARS:
            problems.append(
                f"**{heading}** needs a bit more detail "
                f"({len(content)} characters, {MIN_SECTION_CHARS} expected)."
            )
    return problems


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: check_pr_body.py <path-to-body-file>", file=sys.stderr)
        return 2

    with open(sys.argv[1], encoding="utf-8") as handle:
        body = handle.read()

    problems = check(body)
    if not problems:
        print("Pull request description looks complete.")
        return 0

    print("Some of the pull request description still needs filling in:\n")
    for problem in problems:
        print(f"- {problem}")
    print(
        "\nEdit the description and this check re-runs on its own. "
        "The sections exist because they are the parts a reviewer cannot get "
        "from the diff: why the change is needed, how you know it works, and "
        "what breaks if it is wrong."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
