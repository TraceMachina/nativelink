// Copyright 2023 The NativeLink Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// This script is designed to be run as a Google Cloud Function. It will
// update the DNS records for the private ip of the scheduler instance.
// This is used because the scheduler instance might be replaced under
// rare circumstances, and we want to make sure that the DNS records are
// updated to point to the new instances.

const functions = require('@google-cloud/functions-framework');
const compute = require('@google-cloud/compute');
const { DNS } = require('@google-cloud/dns');

const projectId = process.env.GCP_PROJECT || process.env.GCLOUD_PROJECT;
const dnsZone = process.env.DNS_ZONE;
const schedulerInstancePrefix = process.env.SCHEDULER_INSTANCE_PREFIX;

async function getRecordIfExists(zone, name) {
  const records = await zone.getRecords({
    name,
    type: 'A'
  });
  if (!records.length || !records[0].length) {
    return null;
  }
  return records[0][0];
}

async function findSchedulerInstance() {
  const instancesClient = new compute.InstancesClient({ projectId });

  const aggListRequest = instancesClient.aggregatedListAsync({
    project: projectId,
    maxResults: 5,
    filter: `status=RUNNING AND name=${schedulerInstancePrefix}*`,
  });

  let schedulerInstance = null;

  for await (const [_zone, instancesObject] of aggListRequest) {
    if (!instancesObject.instances || !instancesObject.instances.length) {
      continue;
    }
    for (const instance of instancesObject.instances) {
      schedulerInstance = instance;
    }
  }
  return schedulerInstance;
}

async function updateIPs(_cloudEvent) {
  const schedulerInstance = await findSchedulerInstance();

  const dns = new DNS({ projectId });
  const zone = dns.zone(dnsZone);
  const dnsName = (await zone.get())[1].dnsName;
  const addRecords = [];
  const deleteRecords = [];

  if (schedulerInstance) {
    // Note: In the event the network that the scheduler and workers communicate through is
    // different this section should be updated.
    const internalForWorkerSchedulerIp = schedulerInstance.networkInterfaces[0].networkIP;
    const existingInternalForWorkerSchedulerRecord = await getRecordIfExists(zone, `worker_api.scheduler.${dnsName}`);
    if (!existingInternalForWorkerSchedulerRecord || existingInternalForWorkerSchedulerRecord.data[0] !== internalForWorkerSchedulerIp) {
      if (existingInternalForWorkerSchedulerRecord) {
        deleteRecords.push(existingInternalForWorkerSchedulerRecord);
      }
      addRecords.push(zone.record("A", {
        name: `worker_api.scheduler.${dnsName}`,
        ttl: 300,
        data: internalForWorkerSchedulerIp,
      }));
    }
  }

  if (!addRecords.length) {
    // Nothing to change.
    console.log('nothing to do');
    return;
  }

  const result = await zone.createChange({ add: addRecords, delete: deleteRecords });

  const change = result[0];
  console.log("Changes", { add: addRecords, delete: deleteRecords });
}

functions.cloudEvent('updateIPs', updateIPs);
