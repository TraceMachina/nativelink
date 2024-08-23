import { transformMarkdownToMdx } from "./markdown";

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

export type ConvertFile = {
  input: string;
  output: string;
  docs: {
    title: string;
    description: string;
    pagefind?: boolean;
    assets?: string[];
  };
};

export async function convertMarkdownToMdx({
  input,
  output,
  docs,
}: ConvertFile): Promise<void> {
  try {
    const markdown = await readMarkdownFile(input);
    const mdxContent = await transformMarkdownToMdx(markdown, docs);
    await writeMdxFile(output, mdxContent);
  } catch (err) {
    console.error(`Error during conversion: ${err}`);
    throw err;
  }
}
