export interface BuildEvent extends protobuf.Message {
    orderedBuildEvent: {
      streamId: {
        buildId: string;
        invocationId: string;
      },
      event: {
        eventTime: {
          seconds: protobuf.Long;
          nanos: number;
        };
        // biome-ignore lint/suspicious/noExplicitAny: Not known yet
        bazelEvent?: any;
      };
    };
  }

 export type ParsedMessage = {
    prefix: string;
    eventType: string;
    eventID: string;
    subEventID: string;
    sequenceNumber: string;
}


  export type Progress = {
    stderr: string;
  };
