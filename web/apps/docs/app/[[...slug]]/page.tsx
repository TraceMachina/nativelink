import { source } from "@/lib/source";
import { getMDXComponents } from "@/mdx-components";
import { AskAi } from "@nativelink/ui";
import { DocsBody, DocsDescription, DocsPage, DocsTitle } from "fumadocs-ui/page";
import type { Metadata } from "next";
import { notFound } from "next/navigation";

const DOCS_URL = "https://docs.nativelink.com";

export default async function Page(props: {
  params: Promise<{ slug?: string[] }>;
}) {
  const params = await props.params;
  const page = source.getPage(params.slug);
  if (!page) notFound();

  const MDX = page.data.body;
  const pageUrl = `${DOCS_URL}${page.url}`;

  return (
    <DocsPage toc={page.data.toc} full={page.data.full}>
      <DocsTitle>{page.data.title}</DocsTitle>
      <DocsDescription>{page.data.description}</DocsDescription>
      <DocsBody>
        <MDX components={getMDXComponents()} />
      </DocsBody>
      {/* Hand this page to an assistant, or fetch the corpus it belongs to.
       *  Sits between the body and the auto-rendered prev/next footer. */}
      <AskAi
        size="sm"
        siteUrl={DOCS_URL}
        prompt={`Read ${DOCS_URL}/llms.txt, then explain this NativeLink documentation page and answer my questions about it: ${pageUrl}`}
        className="not-prose mt-12 mb-8 border-t border-border pt-8"
      />
    </DocsPage>
  );
}

export async function generateStaticParams() {
  return source.generateParams();
}

export async function generateMetadata(props: {
  params: Promise<{ slug?: string[] }>;
}): Promise<Metadata> {
  const params = await props.params;
  const page = source.getPage(params.slug);
  if (!page) notFound();

  return {
    title: page.data.title,
    description: page.data.description,
  };
}
