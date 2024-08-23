import { generateDocs } from "./metaphase_aot";

// Only run if this file is being executed directly
if (import.meta.main) {
  await generateDocs({
    crateDataPath:
      "../../../bazel-bin/nativelink-config/docs_json.rustdoc/nativelink_config.json",
    outputPath: "../content/docs/reference/nativelink-config.mdx",
  });
}
