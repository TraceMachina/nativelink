import { transformMarkdownToMdx } from "./md_to_mdx";

async function readMarkdownFile(filePath: string): Promise<string> {
  try {
    return await Bun.file(filePath).text();
  } catch (err) {
    console.error(`Error reading file: ${err}`);
    throw err;
  }
}

async function writeMdxFile(
  outputFilePath: string,
  mdxContent: string,
): Promise<void> {
  try {
    await Bun.write(outputFilePath, mdxContent);
    console.info(`Generated: ${outputFilePath}`);
  } catch (err) {
    console.error(`Error writing file: ${err}`);
    throw err;
  }
}

async function convertMarkdownToMdx(file: ConvertFileType): Promise<void> {
  try {
    const markdown = await readMarkdownFile(file.input);
    const mdxContent = await transformMarkdownToMdx(markdown, file.docs);
    await writeMdxFile(file.output, mdxContent);
  } catch (err) {
    console.error(`Error during conversion: ${err}`);
    throw err;
  }
}

export type ConvertFileType = {
  input: string;
  output: string;
  docs: {
    title: string;
    description: string;
    pagefind?: boolean;
    assets?: string[];
  };
};

// Directories
const rootDir = "../..";
const docsDir = "src/content/docs/docs";
const assetsDir = "@assets";

const filesToConvert: ConvertFileType[] = [
  {
    input: `${rootDir}/local-remote-execution/README.md`,
    output: `${docsDir}/explanations/lre.mdx`,
    docs: {
      title: "Local Remote Execution",
      description: "Local Remote Execution architecture",
    },
  },
  {
    input: `${rootDir}/CONTRIBUTING.md`,
    output: `${docsDir}/contribute/guidelines.mdx`,
    docs: {
      title: "NativeLink contribution guidelines",
      description: "Contribution Guidelines",
    },
  },
  {
    input: "README.md",
    output: `${docsDir}/contribute/docs.mdx`,
    docs: {
      title: "The NativeLink documentation",
      description: "Working on documentation",
    },
  },
  {
    input: `${rootDir}/nativelink-config/README.md`,
    output: `${docsDir}/config/configuration-intro.mdx`,
    docs: {
      title: "NativeLink configuration guide",
      description: "NativeLink configuration guide",
    },
  },
  {
    input: `${rootDir}/deployment-examples/chromium/README.md`,
    output: `${docsDir}/deployment-examples/chromium.mdx`,
    docs: {
      title: "NativeLink deployment example for Chromium",
      description: "NativeLink deployment example for Chromium",
    },
  },
  {
    input: `${rootDir}/deployment-examples/kubernetes/README.md`,
    output: `${docsDir}/deployment-examples/kubernetes.mdx`,
    docs: {
      title: "Local Remote Execution architecture",
      description: "Local Remote Execution architecture",
    },
  },
  {
    input: `${rootDir}/CHANGELOG.md`,
    output: `${docsDir}/reference/changelog.mdx`,
    docs: {
      title: "Changelog",
      description: "NativeLink's Changelog",
      pagefind: false, // Set pagefind to false for changelog
    },
  },
  {
    input: `${rootDir}/README.md`,
    output: `${docsDir}/introduction/setup.mdx`,
    docs: {
      title: "Introduction",
      description: "Get started with NativeLink",
      pagefind: true,
      assets: [`${assetsDir}/logo-dark.svg`, `${assetsDir}/logo-light.svg`],
    },
  },
];

filesToConvert.map((file) => {
  convertMarkdownToMdx(file);
});
