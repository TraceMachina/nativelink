type ParsedMessage = {
  prefix: string;
  eventType: string;
  eventID: string;
  subEventID: string;
  sequenceNumber: string;
}

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

export function constructRedisKey(parsedMessage: ParsedMessage) {
  console.log("\nNew Published Event: ")
  console.log("  EventID: ", parsedMessage.eventID)
  console.log("  Sequence Number: ", parsedMessage.sequenceNumber)
  console.log("  Invocation ID: ", parsedMessage.subEventID)
  console.log("------------------")
  return `${parsedMessage.prefix}:${parsedMessage.eventType}:${parsedMessage.eventID}:${parsedMessage.subEventID}:${parsedMessage.sequenceNumber}`;
}
