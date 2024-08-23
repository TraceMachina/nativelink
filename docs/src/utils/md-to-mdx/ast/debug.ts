import type { Literal, Node, Parent, Root, RootContent } from "mdast";

// Define a type for nodes that contain a `value` property
interface HtmlNode extends Literal {
  type: "html";
  value: string;
}

// This function logs all nodes of a given type in the AST.
export function logSpecificNodes(tree: Root, nodeType: string): void {
  tree.children.forEach((node, _index) => {
    if (node.type === nodeType) {
      console.info(node);
    }
  });
}

// This function searches for HTML nodes with a specific ID and logs them.
export function findNodesById(tree: RootContent[], targetId: string): void {
  const idRegex = new RegExp(`id=["']${targetId}["']`, "i");
  //   console.log(tree.children[1]);
  tree.forEach((node, _index) => {
    if (node.type === "html" && idRegex.test((node as HtmlNode).value)) {
      console.info(node);
    }
  });
}

// Exporting the AST to a file
// This function writes the entire AST or a portion of it to a file for easier inspection.
export function exportAstToFile(tree: Root, filename: string): void {
  Bun.write(filename, JSON.stringify(tree, null, 2));
}

// Recursively logging the AST up to a certain depth
// This function logs the AST up to a specified depth, which is useful for large structures.
export function logAstAtDepth(
  node: Node,
  depth: number,
  maxDepth: number,
): void {
  if (depth > maxDepth) {
    return;
  }

  if ("children" in node && Array.isArray((node as Parent).children)) {
    for (const child of (node as Parent).children) {
      logAstAtDepth(child, depth + 1, maxDepth);
      console.info(node);
    }
  }
}

// Example usage (uncomment the relevant lines to use):

// logSpecificNodes(tree, "paragraph");
// findNodesById(tree, "description");
// exportAstToFile(tree, 'ast-output.json');
// logAstAtDepth(tree, 0, 2);
