import type {
  Blockquote,
  Code,
  Heading,
  Image,
  InlineCode,
  Paragraph,
  PhrasingContent,
  Root,
  RootContent,
  Text,
} from "mdast";
import { visit } from "unist-util-visit";

// import { findNodesById, logFirstNNodes } from "./debug";

const BLOCK_TYPES = ["caution", "note", "tip"];

export function extractTitle(tree: Root): {
  title: string;
  content: RootContent[];
} {
  const { title, index } = extractTitleFromTree(tree);
  const content = removeTitleFromTree(tree, index);
  return { title, content };
}

function extractTitleFromTree(tree: Root): {
  title: string;
  index: number;
} {
  let title = "Default Title";
  let titleIndex = -1;

  for (let i = 0; i < tree.children.length; i++) {
    const node = tree.children[i];
    if (node && node.type === "heading" && (node as Heading).depth === 1) {
      title = extractTextFromNode(node as Heading);
      titleIndex = i;
      break;
    }
  }

  return { title, index: titleIndex };
}

function removeTitleFromTree(tree: Root, index: number): RootContent[] {
  if (index >= 0) {
    return [
      ...tree.children.slice(0, index),
      ...tree.children.slice(index + 1),
    ];
  }
  return tree.children;
}

function extractTextFromNode(node: Heading | Paragraph): string {
  return (node.children as Text[]).map((child) => child.value).join("");
}

export function transformGitHubMarkdown(content: RootContent[]): RootContent[] {
  return content.flatMap((node) => {
    if (node.type === "blockquote") {
      const transformed = transformBlockquote(node as Blockquote);
      if (transformed) {
        return [transformed];
      }
    }
    return [node];
  });
}

function transformBlockquote(blockquote: Blockquote): RootContent | null {
  const firstParagraph = blockquote.children[0] as Paragraph;
  const firstText = extractTextFromNode(firstParagraph).trim();

  const blockType = extractBlockType(firstText);
  if (blockType) {
    const contentText = extractBlockquoteContent(blockquote);
    const cleanedContentText = cleanBlockTypeFromContent(
      contentText,
      blockType,
      firstText.match(/^\[\!(\w+)\]/)?.[1] || "",
    );

    return {
      type: "html",
      value: `:::${blockType}\n${cleanedContentText}\n:::`,
    };
  }
  return null;
}

function extractBlockType(firstText: string): string | null {
  const match = firstText.match(/^\[\!(\w+)\]/);
  if (match?.[1]) {
    let blockType = match[1];
    if (blockType.toUpperCase() === "WARNING") {
      blockType = "caution";
    }
    blockType = blockType.toLowerCase();
    if (BLOCK_TYPES.includes(blockType)) {
      return blockType;
    }
  }
  return null;
}

function extractBlockquoteContent(blockquote: Blockquote): string {
  return blockquote.children
    .map((paragraph) => {
      if (paragraph.type === "paragraph") {
        return (paragraph as Paragraph).children
          .map((child) => {
            if (child.type === "text") {
              return (child as Text).value;
            }
            if (child.type === "inlineCode") {
              return `\`${(child as InlineCode).value}\``;
            }
            return "";
          })
          .join("");
      }
      if (paragraph.type === "code") {
        return `\`${(paragraph as Code).value}\``;
      }
      return "";
    })
    .join("\n");
}

function cleanBlockTypeFromContent(
  contentText: string,
  blockType: string,
  originalBlockType: string,
): string {
  return contentText
    .replace(`[!${blockType.toUpperCase()}]`, "")
    .replace(`[!${originalBlockType.toUpperCase()}]`, "")
    .trim();
}

export function preserveInlineCode(content: RootContent[]): RootContent[] {
  visit({ type: "root", children: content } as Root, "text", (node: Text) => {
    node.value = node.value.replace(/\\/g, "");
  });
  return content;
}

// Utility to escape HTML entities
function escapeHtml(unsafe: string): string {
  return unsafe
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

// Function to generate asset import statements
export function generateAssetImports(assets: string[]): RootContent[] {
  return assets.map((asset) => {
    const assetName = transformToPascalCase(asset);
    return {
      type: "html",
      value: `import ${assetName} from '${asset}';\n`,
    };
  });
}

// Helper function to convert file paths to PascalCase
function transformToPascalCase(filePath: string): string {
  const fileName = filePath.split("/").pop()?.split(".")[0];
  if (!fileName) {
    throw new Error("Invalid file path");
  }

  return fileName
    .split(/[^a-zA-Z0-9]/)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1).toLowerCase())
    .join("");
}

export function processTarget(
  tree: RootContent[],
  targetId: string,
): RootContent[] {
  const alignRegex = /align=["'](center|left|right)["']/i;

  const content = tree.map((node, index, children) => {
    if (isMatchingTargetNode(node, targetId)) {
      replaceAlignAttributeInNode(node, alignRegex);
      if (targetId === "logo") {
        transformImgSrc(node);
      }
      if (targetId === "badges") {
        ensureBadgeClasses(node);
        processAndWrapNextBlock(children, index);
      }
    }
    return node;
  });

  return content;
}

function ensureBadgeClasses(node: RootContent): void {
  if (isHtmlNode(node)) {
    // Replace the class="" attribute directly in node.value
    node.value = node.value.replace(/class="([^"]*)"/g, (_, classes) => {
      // Ensure the required classes are present
      const requiredClasses = [
        "flex",
        "justify-center",
        "items-center",
        "flex-wrap",
        "gap-0.5",
      ];

      // Split the existing classes into an array
      const existingClasses = classes.trim().split(/\s+/);

      // Add any missing required classes
      for (const requiredClass of requiredClasses) {
        if (!existingClasses.includes(requiredClass)) {
          existingClasses.push(requiredClass);
        }
      }

      // Return the updated class attribute
      return `class="${existingClasses.join(" ")}"`;
    });
  }
}

// Helper function to check if a node matches the target ID
function isMatchingTargetNode(node: RootContent, targetId: string): boolean {
  if (node.type === "html" && typeof node.value === "string") {
    const idRegex = new RegExp(`id=["']${targetId}["']`, "i");
    return idRegex.test(node.value);
  }
  return false;
}

// Function to replace the align attribute with a CSS class
function replaceAlignAttributeInNode(
  node: RootContent,
  alignRegex: RegExp,
): void {
  if (node.type === "html" && typeof node.value === "string") {
    const newValue = node.value.replace(alignRegex, (_, p1) => {
      const alignment = p1.toLowerCase() as keyof typeof alignToClassMap;
      return `class="${alignToClassMap[alignment]}"`;
    });
    node.value = newValue;
  }
}

function transformImgSrc(node: RootContent): void {
  if (isHtmlNode(node)) {
    node.value = node.value
      // Remove the entire <picture> tag and replace it with the two <img> tags
      .replace(
        /<picture>[\s\S]*?<source\s+media="[^"]*dark[^"]*"\s+srcset="[^"]*"[^>]*>\s*<source\s+media="[^"]*light[^"]*"\s+srcset="[^"]*"[^>]*>\s*<img\s+[^>]*src="[^"]*"[^>]*>\s*<\/picture>/g,
        `
        <img alt="NativeLink Logo" class="light:sl-hidden" src={LogoDark.src} width="376" height="100" />
        <img alt="NativeLink Logo" class="dark:sl-hidden !mt-0" src={LogoLight.src} width="376" height="100" />
        `,
      );
  }
}

// Type guard to check if a node is an HTML node with a value property
function isHtmlNode(
  node: RootContent,
): node is RootContent & { value: string } {
  return node.type === "html" && typeof node.value === "string";
}

function wrapLinksInBlocks(blockNode: RootContent): RootContent[] {
  if (blockNode.type !== "paragraph") {
    return [blockNode];
  }

  const block = blockNode as Paragraph;
  let modified = false;

  // Process the children of the paragraph
  const newChildren = block.children.map((child) => {
    if (
      child.type === "link" &&
      child.children.some((c) => c.type === "image")
    ) {
      const image = child.children.find((c) => c.type === "image") as Image;
      if (image) {
        modified = true; // Mark as modified if we change something
        const altText = escapeHtml(image.alt || "");
        const imageUrl = escapeHtml(image.url || "");
        const linkUrl = escapeHtml(child.url || "");

        return {
          type: "html",
          value: `<p>\n[![${altText}](${imageUrl})](${linkUrl})\n</p>`,
        } as RootContent;
      }
    }
    return child; // Return unmodified child if no changes were needed
  });

  if (modified) {
    // If the block was modified, return the new paragraph
    return [
      {
        type: "paragraph",
        children: newChildren.filter((node): node is PhrasingContent => !!node), // Ensure all children are valid PhrasingContent
      },
    ];
  }

  // Return the original block if no changes were made
  return [blockNode];
}

// Function to process and wrap the next block in the children array
function processAndWrapNextBlock(
  children: RootContent[],
  index: number,
): Paragraph | null {
  const nextNode = children[index + 1];

  // Ensure the next node exists and is a paragraph
  if (nextNode && nextNode.type === "paragraph") {
    // console.log(nextNode)
    // Wrap links in blocks if applicable
    const wrappedParagraphs = wrapLinksInBlocks(nextNode);

    // If the paragraph has been modified, replace it in the children array
    if (wrappedParagraphs.length > 0) {
      children.splice(index + 1, 1, ...wrappedParagraphs);
      return wrappedParagraphs[0] as Paragraph;
    }
  }

  // If no wrapping was done, return null to signal no changes
  return null;
}

// Alignments to CSS class mapping
type Alignments = "center" | "left" | "right";
const alignToClassMap: Record<Alignments, string> = {
  center: "flex justify-center items-center",
  left: "flex justify-start items-center",
  right: "flex justify-end items-center",
};
