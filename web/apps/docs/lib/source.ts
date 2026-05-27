import { docs } from "@/.source";
import { loader } from "fumadocs-core/source";

// baseUrl is "/" because Next.js basePath ("/docs") prepends to every
// next/link href automatically. If we put "/docs" here as well, the
// sidebar would emit /docs/foo and next/link would double-prefix it to
// /docs/docs/foo.
export const source = loader({
  baseUrl: "/",
  source: docs.toFumadocsSource(),
});
