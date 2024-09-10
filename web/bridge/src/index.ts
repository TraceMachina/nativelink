import { initializeRedisClients } from './redis';
import { initializeProtobuf } from './protobuf';
import { handleEvent } from './eventHandler';
import { startWebSocket } from './websocket';
import { startWebServer } from './http';


export async function start() {
  // Base URL
  const github = "https://raw.githubusercontent.com"

  // NativeLink URL
  const nativelinkRepo = "TraceMachina/nativelink"
  const nativelinkBranch = "main"
  const nativelinkProtoPath = `${github}/${nativelinkRepo}/${nativelinkBranch}/nativelink-proto/`;

  // Proto Remote Path
  const protoRepo = "protocolbuffers/protobuf"
  const protoBranch = "master"
  const protoRepoPath = `${github}/${protoRepo}/${protoBranch}/main/src/google/protobuf`;
  const protoDevToolsPath = `${github}/${protoRepo}/main/src/google/devtools/build/v1`;

  const googleProto = "googleapis/googleapis"
  const googleProtoBranch = "master"
  const googleProtoPath = `${github}/${googleProto}/${googleProtoBranch}/google/devtools/build/v1`;

    // Bazel Remote Path
  const bazelRepo = "bazelbuild/bazel"
  const bazelBranch = "master"
  const bazelProtoPath = `${github}/${bazelRepo}/${bazelBranch}/src/main/java/com/google/devtools/build/lib/buildeventstream/proto`;

  // TODO(SchahinRohani): Add Buck2 Protos for future Buck2 support
  // Buck2 Protos
  // const buck2Repo = "facebook/buck2/main"
  // const buck2Branch = "main"
  // const buck2ProtoPath = `${github}/${buck2Repo}/${buck2Branch}/app/buck2_data/data.proto`;

  // Actual using Protos.
  const PublishBuildEventProto =`${googleProtoPath}/publish_build_event.proto`;
  const BazelBuildEventStreamProto = `${bazelProtoPath}/build_event_stream.proto`;

  const protos = [ PublishBuildEventProto, BazelBuildEventStreamProto ]

  console.info("Link to: \n")
  console.info("Google Publish Build Events Proto:\n", PublishBuildEventProto, "\n");
  console.info("Bazel Build Event Stream Proto:\n", BazelBuildEventStreamProto, "\n")

  // Load Remote Bazel Proto Files
  const protoTypes = await initializeProtobuf(protos)

  const { redisClient, commandClient } = await initializeRedisClients();

  // Subscribe to the build_event channel
  await redisClient.subscribe(process.env.NATIVELINK_PUB_SUB_CHANNEL || "build_event", async (message: string) => {
    await handleEvent(message, commandClient, protoTypes);
  });

  const websocketServer = startWebSocket();
  const webServer = startWebServer();

  process.on('SIGINT', async () => {
    await redisClient.disconnect();
    await commandClient.disconnect();
    console.info("Received SIGINT. Shutdown gracefully.")
    process.exit();
  });
  process.on('SIGTERM', async () => {
    await redisClient.disconnect();
    await commandClient.disconnect();
    console.info("Received SIGTERM. Shutdown gracefully.")
    process.exit();
  });
}
