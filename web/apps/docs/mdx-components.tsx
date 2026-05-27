import { Callout as FumadocsCallout } from "fumadocs-ui/components/callout";
import { Tab, Tabs } from "fumadocs-ui/components/tabs";
import defaultMdxComponents from "fumadocs-ui/mdx";
import type { MDXComponents } from "mdx/types";
import { Callout } from "@/components/callout";
import { Mermaid } from "@/components/mermaid";
import { Steps } from "@/components/steps";

export function getMDXComponents(components?: MDXComponents): MDXComponents {
  return {
    ...defaultMdxComponents,
    // Our themed Callout takes precedence over Fumadocs's default;
    // expose Fumadocs's as FumaCallout if anyone wants the original style.
    Callout,
    FumaCallout: FumadocsCallout,
    Steps,
    Tabs,
    Tab,
    Mermaid,
    ...components,
  };
}
