import protobuf from 'protobufjs';

export async function initializeProtobuf(protos: string[]) {
    console.log("Loading Remote Proto Files");

    const combinedRoot = new protobuf.Root();
    const loadedFiles: Record<string, boolean> = {};
    const processedImports = new Set<string>();
    for (const proto of protos) {
        await loadProto(loadedFiles, combinedRoot, proto, processedImports);
    }
    console.log("\nDone parsing all proto files.\n");
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
    if (importPath.startsWith("google/api") || importPath.startsWith("google/devtools/build/v1")) {
        return `https://raw.githubusercontent.com/googleapis/googleapis/1f2e5aab4f95b9bd38dd1ac8c7486657f93c1975/${importPath}`;
    }

    if (importPath.startsWith("google/protobuf")) {
        return `https://raw.githubusercontent.com/protocolbuffers/protobuf/v29.0-rc2/src/${importPath}`;
    }

    if (importPath.includes("com/google/devtools/build/lib/packages/metrics") || importPath.startsWith("src/main/protobuf")) {
        return `https://raw.githubusercontent.com/bazelbuild/bazel/9.0.0-pre.20241023.1/${importPath}`;
    }
    return new URL(importPath, protoUrl).toString();
}

async function loadProto(
    loadedFiles: Record<string, boolean>,
    root: protobuf.Root,
    protoUrl: string,
    processedImports: Set<string>,
    indentLevel = 0,
) {
    if (loadedFiles[protoUrl]) {
        return;
    }
    const response = await fetch(protoUrl);
    if (!response.ok) {
        throw new Error(`Failed to fetch .proto file from ${protoUrl}: ${response.statusText}`);
    }

    const parsedProto = protobuf.parse(await response.text(), root);
    loadedFiles[protoUrl] = true;
    if (indentLevel < 1) {
        console.log(`\n${ '  '.repeat(indentLevel)} ${protoUrl}:`);
    }
    if (parsedProto.imports && parsedProto.imports.length > 0) {
        for (const importPath of parsedProto.imports) {
            const resolvedImportUrl = resolveImportPath(protoUrl, importPath);
            if (!processedImports.has(resolvedImportUrl)) {
                console.log(`${ '  '.repeat(indentLevel)}  - ${importPath}`);
                processedImports.add(resolvedImportUrl);
                await loadProto(loadedFiles, root, resolvedImportUrl, processedImports, indentLevel + 1,);
            }
        }
    }
}
