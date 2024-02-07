# Copyright 2023 The NativeLink Authors. All rights reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
"""Publishes cloud monitoring metrics from Prometheus metrics.

This script is intended to be run as a cron job on the scheduler instance.
It will read the Prometheus metrics from the scheduler and publish them
to Google Cloud Monitoring.
"""
import sys
import urllib.request
import time
import argparse
from google.cloud import monitoring_v3


def publish_google_cloud_monitoring(metrics, project_id):
    client = monitoring_v3.MetricServiceClient()

    time_series = []

    for metric_name in metrics:
        if metrics[metric_name] is None:
            continue
        now = time.time()
        seconds = int(now)
        nanos = int((now - seconds) * 10 ** 9)

        if isinstance(metrics[metric_name], float):
            value = {
                "double_value": metrics[metric_name],
            }
        else:
            value = {
                "int64_value": metrics[metric_name],
            }

        series = monitoring_v3.TimeSeries()
        # TODO(allada) This should be a variable (x86_workers).
        series.metric.type = f"custom.googleapis.com/nativelink/x86_workers/{metric_name}"
        series.points = [{
            "interval": monitoring_v3.TimeInterval({
                "start_time": {
                    "seconds": seconds,
                    "nanos": nanos,
                },
                "end_time": {
                    "seconds": seconds,
                    "nanos": nanos,
                }
            }),
            "value": value,
        }]

        time_series.append(series)

    client.create_time_series(
        request = {
            "name": f"projects/{project_id}",
            "time_series": time_series,
        }
    )


def get_metric_value(line):
    if line.startswith(b'#'):
        return None
    parts = line.decode('ascii').split(' ', 1)
    if len(parts) != 2:
        print(f"Invalid line: {line}", file=sys.stderr)
        return None
    return (parts[0].strip(), int(float(parts[1].strip())))


def main():
    req = urllib.request.Request("http://metadata.google.internal/computeMetadata/v1/instance/attributes/nativelink-type")
    req.add_header("Metadata-Flavor", "Google")
    instance_type = urllib.request.urlopen(req).read()
    # Only the scheduler should publish metrics.
    if instance_type != b"scheduler":
        return

    parser = argparse.ArgumentParser(
        description='Publish Prometheus metrics to cloud services.')
    parser.add_argument('--url', type=str, required=True)
    parser.add_argument('--project', type=str, required=True)

    args = parser.parse_args()

    active_actions_total = None
    queued_actions_total = None
    available_cpus = None

    lines = urllib.request.urlopen(args.url).readlines()
    for line in lines:
        parts = get_metric_value(line)
        if parts is None:
            continue
        (metric_name, value) = parts
        # TODO(#384) Prometheus has a bug where it duplicates names a lot, when it's fixed this must be changed.
        if metric_name.startswith("NATIVELINK_NATIVELINK_schedulers_NATIVELINK_schedulers_MAIN_SCHEDULER_active_actions_total"):
            active_actions_total = value
        if metric_name.startswith("NATIVELINK_NATIVELINK_schedulers_NATIVELINK_schedulers_MAIN_SCHEDULER_queued_actions_total"):
            queued_actions_total = value
        if metric_name.startswith("NATIVELINK_NATIVELINK_schedulers_NATIVELINK_schedulers_MAIN_SCHEDULER_cpu_count_available_properties"):
            available_cpus = value

    all_cpus = None
    if available_cpus is not None and active_actions_total is not None:
        all_cpus = float(available_cpus + active_actions_total)

    actions_total = None
    if active_actions_total is not None and queued_actions_total is not None:
        actions_total = active_actions_total + queued_actions_total

    # We need to use something smaller than infinity but large enough it'll allocate some nodes
    # in the event there are no instances online.
    cpu_ratio = 2.0 if queued_actions_total is not None else None
    if all_cpus is not None and all_cpus != 0 and actions_total is not None:
        cpu_ratio = float(actions_total) / all_cpus

    publish_google_cloud_monitoring({
        "actions_active": active_actions_total,
        "actions_queued": queued_actions_total,
        "cpu_total": available_cpus,

        "actions_total": actions_total,
        "cpu_ratio": cpu_ratio,
    }, args.project)


if __name__ == "__main__":
    main()
