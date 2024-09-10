import type protobuf from 'protobufjs';
import { commandOptions, type RedisClientType } from 'redis';
import { constructRedisKey, parseMessage } from './utils';
import { broadcastProgress } from './websocket';

interface CustomBuildEvent extends protobuf.Message {
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
//   console.log(`Received message from build_event channel: ${message}`);

  const parsedMessage = parseMessage(message);
//   console.log('Parsed Message:', parsedMessage);

  const redisKey = constructRedisKey(parsedMessage);
//   console.log('Constructed Redis Key:', redisKey);

  switch (parsedMessage.eventType) {
    case 'LifecycleEvent':
    //   console.log(`Processing ${parsedMessage.eventType} with key ${redisKey}`);
      await fetchAndDecodeBuildData(redisKey, commandClient, types.PublishLifecycleEventRequest);
      break;
    case 'BuildToolEventStream':
    //   console.log(`Processing ${parsedMessage.eventType} with key ${redisKey}`);
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
      // console.log(`Fetched build data for key ${redisKey}`);
      const buffer = Buffer.from(buildData);
      const decodedMessage = messageType.decode(buffer) as CustomBuildEvent;

      // Hier wird der `bazelEvent` dekodiert, falls er existiert
      if(decodedMessage.orderedBuildEvent) {


        const buildId = decodedMessage.orderedBuildEvent.streamId.buildId
        const invocationId = decodedMessage.orderedBuildEvent.streamId.invocationId
        // const sequenceNumber = decodedMessage.orderedBuildEvent

        console.log("Build ID: ", buildId)
        console.log("Invocation ID: ", invocationId)
        // console.log("Sequence Number: ", sequenceNumber)


        const eventTime = decodedMessage.orderedBuildEvent.event.eventTime;

        // Convert seconds to milliseconds and add nanoseconds converted to milliseconds
        const milliseconds = eventTime.seconds.low * 1000 + Math.floor(eventTime.nanos / 1000000);

        // Create a new Date object using the computed milliseconds
        const eventDate = new Date(milliseconds);

        // const date = new Date(decodedMessage.orderedBuildEvent.event.eventTime.seconds*1000);
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
        // console.log("Got here.")
        const decodedBazelEvent = decodeBazelEvent(decodedMessage.orderedBuildEvent.event.bazelEvent, messageType.root);
        // console.log("Decoded Bazel Event:", decodedBazelEvent);
      } else {
        // console.log("No Bazel Event found.");
      }

    //   console.log("Decoded String:", decodedMessage.toJSON());
    } else {
    //   console.log(`No build data found for key ${redisKey}`);
    }
  } catch (err) {
    // console.error(`Error fetching build data for key ${redisKey}:`, err);
  }
}

// biome-ignore lint/suspicious/noExplicitAny: <explanation>
function decodeBazelEvent(bazelEvent: any, root: protobuf.Root): any {
  if (!bazelEvent || !bazelEvent.value) return null;

  const decodedBinaryData = Buffer.from(bazelEvent.value, 'base64');
  const messageType = root.lookupType(bazelEvent.typeUrl.split('/').pop());
  const decodedMessage = messageType.decode(decodedBinaryData);
  // In ein lesbares JSON-Objekt umwandeln
  const decodedObject = messageType.toObject(decodedMessage, {
    longs: String,
    enums: String,
    bytes: String,
  });

  // Progress Informationen verarbeiten
  if (decodedObject.progress) {
    console.log("Processing progress information...\n\n");
    processProgress(decodedObject.progress);
  }

  return decodedObject;
}

// biome-ignore lint/suspicious/noExplicitAny: <explanation>
function processProgress(progress: any) {
// console.log(progress.stderr)
  if (progress.stderr) {
    // console.log(progress)
    // const cleanStderr = stripAnsi(progress.stderr);
    console.log(progress.stderr);
    broadcastProgress(progress.stderr)
  }

  if (progress.opaqueCount === 1) {
    // console.log(`Progress Opaque Count: ${progress.opaqueCount}`);
    // console.log(progress.stderr);
  }

  if (progress.children) {
    // biome-ignore lint/suspicious/noExplicitAny: <explanation>
    progress.children.forEach((child: any, index: number) => {
    //   console.log(`Child ${index + 1}:`);
      if (child.progress && child.progress.opaqueCount ===2 ) {
        // console.log(`  Child Progress Opaque Count: ${child.progress.opaqueCount}`);
      }
    //   if (child.configuration && child.configuration.id) {
    //     // console.log(`  Child Configuration ID: ${child.configuration.id}`);
    //   }
    });
  }
}
