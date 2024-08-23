import { type ConvertFile, convertMarkdownToMdx } from "./md-to-mdx";

const rootDir = "../";
const docsDir = "src/content/docs";
const assetsDir = "@assets";

const filesToConvert: ConvertFile[] = [
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
