import protobuf from 'protobufjs';

export async function initializeProtobuf(protos: string[]) {
    console.log("Loading Remote Proto Files");

    // Create a new Root instance
    const combinedRoot = new protobuf.Root();

    // Track loaded files to avoid circular dependencies
    const loadedFiles: Record<string, boolean> = {};

    // Track processed imports to avoid duplicates
    const processedImports = new Set<string>();

    // Load all initial proto files
    for (const proto of protos) {
        await loadProto(loadedFiles, combinedRoot, proto, processedImports);
    }

    console.log("\nDone parsing all proto files.\n");

    // Now combinedRoot contains your parsed .proto content
    // Example: Look up specific message types
    const BazelBuildEvent = combinedRoot.lookupType("build_event_stream.BuildEvent");
    const PublishBuildToolEventStreamRequest = combinedRoot.lookupType("google.devtools.build.v1.PublishBuildToolEventStreamRequest");
    const PublishLifecycleEventRequest = combinedRoot.lookupType("google.devtools.build.v1.PublishLifecycleEventRequest");

    console.log("Loaded Types:\n");
    console.log({
      PublishLifecycleEventRequest: PublishLifecycleEventRequest ? PublishLifecycleEventRequest.fullName : "Not found",
      PublishBuildToolEventStreamRequest: PublishBuildToolEventStreamRequest ? PublishBuildToolEventStreamRequest.fullName : "Not found",
      BazelBuildEvent: BazelBuildEvent ? BazelBuildEvent.fullName : "Not found"
    });

    return {
        PublishLifecycleEventRequest,
        PublishBuildToolEventStreamRequest,
        BazelBuildEvent
    };
}

function resolveImportPath(protoUrl: string, importPath: string): string {
    // Handle googleapis imports
    if (importPath.startsWith("google/api") || importPath.startsWith("google/devtools/build/v1")) {
        return `https://raw.githubusercontent.com/googleapis/googleapis/master/${importPath}`;
    }

    // Handle protocolbuffers imports
    if (importPath.startsWith("google/protobuf")) {
        return `https://raw.githubusercontent.com/protocolbuffers/protobuf/master/src/${importPath}`;
    }

    // Handle specific case for bazel
    if (importPath.includes("com/google/devtools/build/lib/packages/metrics") || importPath.startsWith("src/main/protobuf")) {
        return `https://raw.githubusercontent.com/bazelbuild/bazel/master/${importPath}`;
    }

    // Default behavior for other imports - resolve relative to protoUrl
    return new URL(importPath, protoUrl).toString();
}

// Recursive function to fetch, parse, and handle imports
async function loadProto(
    loadedFiles: Record<string, boolean>,
    root: protobuf.Root,
    protoUrl: string,
    processedImports: Set<string>,
    indentLevel = 0,
) {
    if (loadedFiles[protoUrl]) {
        // If already loaded, skip to prevent circular imports
        return;
    }

    const indent = '  '.repeat(indentLevel);

    // Fetch the .proto file content
    const response = await fetch(protoUrl);
    if (!response.ok) {
        throw new Error(`Failed to fetch .proto file from ${protoUrl}: ${response.statusText}`);
    }

    const protoContent = await response.text();

    // Parse the proto content
    const parsedProto = protobuf.parse(protoContent, root);

    // Mark this proto as loaded
    loadedFiles[protoUrl] = true;

    // Log the imports necessary for this proto file
    if (indentLevel < 1) {
        console.log(`\n${indent} ${protoUrl}:`);
    }

    if (parsedProto.imports && parsedProto.imports.length > 0) {
        for (const importPath of parsedProto.imports) {
            const resolvedImportUrl = resolveImportPath(protoUrl, importPath);
            if (!processedImports.has(resolvedImportUrl)) {
                console.log(`${indent}  - ${importPath}`);
                processedImports.add(resolvedImportUrl);
                // Recursively handle the imports
                await loadProto(loadedFiles, root, resolvedImportUrl, processedImports, indentLevel + 1,);
            }
        }
    }
}
