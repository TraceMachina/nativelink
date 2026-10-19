import { docs } from "@/.source/server";
import { loader } from "fumadocs-core/source";

export const source = loader({
  baseUrl: "/",
  // TODO(palfrey): any type is inaccurate, but docs somehow resolves to "never" right now
  source: (docs as any).toFumadocsSource(),
});
