export function parseMessage(message: string) {
  const parts = message.split(':');
  const [prefix, eventType, eventID, subEventID, sequenceNumber] = parts;
  return {
    prefix,
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
  return `${parsedMessage.prefix}:${parsedMessage.eventType}:${parsedMessage.eventID}:${parsedMessage.subEventID}:${parsedMessage.sequenceNumber}`;
}
