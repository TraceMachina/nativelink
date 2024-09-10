import type protobuf from 'protobufjs';
import { commandOptions, type RedisClientType } from 'redis';
import { constructRedisKey, parseMessage } from './utils';
import { broadcastProgress } from './websocket';

interface BuildEvent extends protobuf.Message {
  orderedBuildEvent?: {
    event: {
      eventTime: {
        seconds: protobuf.Long;
        nanos: number;
      };
      // biome-ignore lint/suspicious/noExplicitAny: <explanation>
      bazelEvent?: any;
    };
  };
}

// biome-ignore lint/suspicious/noExplicitAny: <explanation>
export async function handleEvent(message: string, commandClient: any, types: { PublishBuildToolEventStreamRequest: protobuf.Type, PublishLifecycleEventRequest: protobuf.Type }) {
  const parsedMessage = parseMessage(message);
  const redisKey = constructRedisKey(parsedMessage);
  switch (parsedMessage.eventType) {
    case 'LifecycleEvent':
      await fetchAndDecodeBuildData(redisKey, commandClient, types.PublishLifecycleEventRequest);
      break;
    case 'BuildToolEventStream':
      await fetchAndDecodeBuildData(redisKey, commandClient, types.PublishBuildToolEventStreamRequest);
      break;
    default:
      console.log('Unknown event type:', parsedMessage.eventType);
  }
}

// biome-ignore lint/suspicious/noExplicitAny: <explanation>
async function fetchAndDecodeBuildData(redisKey: string, commandClient: any, messageType: protobuf.Type) {
  try {
    const buildData = await commandClient.get(commandOptions({ returnBuffers: true }), redisKey);
    if (buildData) {
      const buffer = Buffer.from(buildData);
      const decodedMessage = messageType.decode(buffer) as BuildEvent;
      if(decodedMessage.orderedBuildEvent) {
        const buildId = decodedMessage.orderedBuildEvent.streamId.buildId
        const invocationId = decodedMessage.orderedBuildEvent.streamId.invocationId
        console.log("Build ID: ", buildId)
        console.log("Invocation ID: ", invocationId)
        const eventTime = decodedMessage.orderedBuildEvent.event.eventTime;
        // Convert seconds to milliseconds and add nanoseconds converted to milliseconds
        const milliseconds = eventTime.seconds.low * 1000 + Math.floor(eventTime.nanos / 1000000);
        // Create a new Date object using the computed milliseconds
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
        const decodedBazelEvent = decodeBazelEvent(decodedMessage.orderedBuildEvent.event.bazelEvent, messageType.root);
      }
    }
  } catch (err) {
    console.error(`Error fetching build data for key ${redisKey}:`, err);
  }
}

// biome-ignore lint/suspicious/noExplicitAny: <explanation>
function decodeBazelEvent(bazelEvent: any, root: protobuf.Root): any {
  if (!bazelEvent || !bazelEvent.value) return null;
  const decodedBinaryData = Buffer.from(bazelEvent.value, 'base64');
  const messageType = root.lookupType(bazelEvent.typeUrl.split('/').pop());
  const decodedMessage = messageType.decode(decodedBinaryData);
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

// biome-ignore lint/suspicious/noExplicitAny: <explanation>
function processProgress(progress: any) {
  if (progress.stderr) {
    console.log(progress.stderr);
    broadcastProgress(progress.stderr)
  }
}
