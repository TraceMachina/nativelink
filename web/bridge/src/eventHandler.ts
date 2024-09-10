import type protobuf from 'protobufjs';
import type { BuildEvent, Progress } from './types/buildTypes';
import { commandOptions, type RedisClientType } from 'redis';
import { constructRedisKey, parseMessage } from './utils';
import { broadcastProgress } from './websocket';


export async function handleEvent(message: string, commandClient: RedisClientType, types: { PublishBuildToolEventStreamRequest: protobuf.Type, PublishLifecycleEventRequest: protobuf.Type }) {
  switch (parseMessage(message).eventType) {
    case 'LifecycleEvent':
      await fetchAndDecodeBuildData(constructRedisKey(parseMessage(message)), commandClient, types.PublishLifecycleEventRequest);
      break;
    case 'BuildToolEventStream':
      await fetchAndDecodeBuildData(constructRedisKey(parseMessage(message)), commandClient, types.PublishBuildToolEventStreamRequest);
      break;
    default:
      console.log('Unknown event type:', parseMessage(message).eventType);
  }
}

async function fetchAndDecodeBuildData(redisKey: string, commandClient: RedisClientType, messageType: protobuf.Type) {
  try {
    const buildData = await commandClient.get(commandOptions({ returnBuffers: true }), redisKey);
    if (buildData) {
      const decodedMessage = messageType.decode(new Uint8Array(Buffer.from(buildData))) as BuildEvent;
      if(decodedMessage.orderedBuildEvent) {
        const buildId = decodedMessage.orderedBuildEvent.streamId.buildId
        const invocationId = decodedMessage.orderedBuildEvent.streamId.invocationId
        console.log("Build ID: ", buildId)
        console.log("Invocation ID: ", invocationId)
        const eventTime = decodedMessage.orderedBuildEvent.event.eventTime;
        const milliseconds = eventTime.seconds.low * 1000 + Math.floor(eventTime.nanos / 1000000);
        const eventDate = new Date(milliseconds);
        console.log("Event time nanos:", eventTime.nanos)
        console.log("Event time seconds:", eventTime.seconds.low)
        console.log("Event time:", eventDate.toISOString());
        const currentTime = new Date()
        const elapsedTime = currentTime.getTime() - eventDate.getTime();
        console.log("Time Now:  ", currentTime.toISOString())
        console.log(`Elapsed Time: ${elapsedTime} ms`);
      }
      if (decodedMessage?.orderedBuildEvent?.event?.bazelEvent) {
        console.log("------------------")
        decodeBazelEvent(decodedMessage.orderedBuildEvent.event.bazelEvent, messageType.root);
      }
    }
  } catch (err) {
    console.error(`Error fetching build data for key ${redisKey}:`, err);
  }
}

// TODO(SchahinRohani): Add Bazel Event Types
// biome-ignore lint/suspicious/noExplicitAny: Bazel Event Types are not known yet
function decodeBazelEvent(bazelEvent: any, root: protobuf.Root): any {
  if (!bazelEvent || !bazelEvent.value) return null;
  const messageType = root.lookupType(bazelEvent.typeUrl.split('/').pop());
  const decodedMessage = messageType.decode(new Uint8Array(Buffer.from(bazelEvent.value, 'base64')));
  const decodedObject = messageType.toObject(decodedMessage, {
    longs: String,
    enums: String,
    bytes: String,
  });
  if (decodedObject.progress) {
    console.log("Processing progress information...\n\n");
    processProgress(decodedObject.progress);
  }
  return decodedObject;
}

function processProgress(progress: Progress) {
  if (progress.stderr) {
    console.log(progress.stderr);
    broadcastProgress(progress.stderr)
  }
}
