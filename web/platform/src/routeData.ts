import { defineRouteMiddleware } from "@astrojs/starlight/route-data";

export const onRequest = defineRouteMiddleware((context) => {
  const { headings } = context.locals.starlightRoute;
  if (
    context.locals.starlightRoute.toc !== undefined &&
    headings.length > 0 &&
    headings[0]?.text.indexOf("Reclient") !== -1
  ) {
    // TODO(palfrey): The tabs setup completely breaks the ToC generation
    // so we override here entirely with a fixed version
    context.locals.starlightRoute.toc.items = [
      {
        depth: 2,
        slug: "_top",
        text: "Overview",
        children: [
          {
            depth: 3,
            slug: "1-setup-reclient-chromium",
            text: "1. Setup Reclient/Chromium",
            children: [],
          },
          {
            depth: 3,
            slug: "2-setup-your-nativelink-configuration-directory",
            text: "2. Setup your NativeLink configuration directory",
            children: [],
          },
          {
            depth: 3,
            slug: "2-setup-your-nativelink-configuration-directory",
            text: "2. Setup your NativeLink configuration directory",
            children: [],
          },
          {
            depth: 3,
            slug: "3-generating-your-mtls-key-files",
            text: "3. Generating your mTLS key files",
            children: [],
          },
          {
            depth: 3,
            slug: "4-upload-the-public-key-to-nativelink",
            text: "4. Upload the public key to NativeLink",
            children: [],
          },
          {
            depth: 3,
            slug: "5-setup-your-environment",
            text: "5. Setup your environment",
            children: [],
          },
          {
            depth: 3,
            slug: "6-build-chromium",
            text: "6. Build Chromium",
            children: [],
          },
          {
            depth: 3,
            slug: "7-watch-the-execution",
            text: "7. Watch the execution",
            children: [],
          },
        ],
      },
    ];
  }
});
