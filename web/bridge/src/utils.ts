export function parseMessage(message: string) {
  const parts = message.split(':');

  const eventType = parts[0].replace('nativelink:', '');
  const eventID = parts.slice(1, 6).join(':');
  const subEventID = parts.slice(6, 11).join(':');
  const sequenceNumber = parts[11];

  return {
    eventType,
    eventID,
    subEventID,
    sequenceNumber
  };
}

// biome-ignore lint/suspicious/noExplicitAny: <explanation>
export function constructRedisKey(parsedMessage: any) {
  console.log("\nNew Published Event: ")
  console.log("  EventID: ", parsedMessage.eventID)
  console.log("  Sequence Number: ", parsedMessage.sequenceNumber)
  console.log("  Invocation ID: ", parsedMessage.subEventID)
  console.log("------------------")

  // console.log( `nativelink:${parsedMessage.eventType}:${parsedMessage.eventID}:${parsedMessage.subEventID}:${parsedMessage.sequenceNumber}`)
  return `nativelink:${parsedMessage.eventType}:${parsedMessage.eventID}:${parsedMessage.subEventID}:${parsedMessage.sequenceNumber}`;
}
