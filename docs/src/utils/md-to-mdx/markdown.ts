import { remark } from "remark";
import remarkMdx from "remark-mdx";
import remarkParse from "remark-parse";
import remarkStringify from "remark-stringify";

import {
  extractTitle,
  generateAssetImports,
  preserveInlineCode,
  processTarget,
  transformGitHubMarkdown,
} from "./ast/transform";

import { parseMarkdown, preProcessMarkdown } from "./raw/transform";

// import { findNodesById, logFirstNNodes } from "./ast/debug";

export type ConvertFile = {
  input: string;
  output: string;
  title: string;
  pagefind?: boolean;
};

export function generateFrontMatter(
  title: string,
  description: string,
  pagefind = true,
): string {
  return `---
title: "${title}"
description: "${description}"
pagefind: ${pagefind ? "true" : "false"}
---
`;
}

type MarkdownProps = {
  title: string;
  description: string;
  pagefind?: boolean;
  assets?: string[];
};

export async function transformMarkdownToMdx(
  input: string,
  docs: MarkdownProps,
): Promise<string> {
  const preprocessedMarkdown = preProcessMarkdown(input);
  const tree = parseMarkdown(preprocessedMarkdown);

  // Extract title and content
  const { content } = extractTitle(tree);

  // Apply transformations in sequence

  // Prepend the import statements to the content
  let transformedContent = docs.assets
    ? [...generateAssetImports(docs.assets), ...content]
    : [...content];

  // // Apply transformations for target IDs
  const targetIds = ["logo", "description", "badges"];
  for (const targetId of targetIds) {
    transformedContent = processTarget(transformedContent, targetId);
  }

  // GitHub Markdown specific transformations
  transformedContent = transformGitHubMarkdown(transformedContent);

  // Preserve inline code
  transformedContent = preserveInlineCode(transformedContent);

  // Reassemble the tree with the transformed content
  tree.children = transformedContent;

  // Convert the transformed tree back to Markdown
  const modifiedMarkdown = remark().use(remarkStringify).stringify(tree);

  // Convert Markdown to MDX
  const processedMdx = await remark()
    .use(remarkParse)
    .use(remarkMdx)
    .use(remarkStringify)
    .process(modifiedMarkdown);

  // Generate front matter
  const frontMatter = generateFrontMatter(
    docs.title,
    docs.description,
    docs.pagefind,
  );

  // Return the final MDX content
  return frontMatter + String(processedMdx);
}
