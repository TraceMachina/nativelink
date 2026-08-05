import { Logo } from "@nativelink/ui";
import type { BaseLayoutProps } from "fumadocs-ui/layouts/shared";

export const baseOptions: BaseLayoutProps = {
  nav: {
    title: <Logo size="md" />,
    url: "/",
    transparentMode: "top",
  },
  // Only the GitHub icon in the top-nav (via githubUrl). Cross-app links back to marketing
  // (/product, /pricing, /resources) clutter a docs-focused nav.
  links: [
  ],
  githubUrl: "https://github.com/TraceMachina/nativelink",
};
