# Concepts

This document describes the concepts behind `nativelink-live`, a real-time live feed viewer for NativeLink.
It's intended to provide insight into the inner workings of NativeLink by displaying live data feeds.

## Key Components

### Live Activity Monitor

This feature offers a real-time, filterable view of all activities occurring within NativeLink.
It allows users to monitor events as they happen, providing immediate insights into system operations.

#### Key Capabilities:

- View real-time activities.
- Filter activities by various criteria (for example, type, status).
  - Type: All, Upload, Download, Clients, Executions, Others
  - Status: All, Success, In Progress, Queued, Failed, Canceled
- Search for specific activities using keywords.
- Clicking the item would show like: Details, `Stdout`, `Stderr` (if applicable), Timestamp.

### Worker Status Dashboard

This dashboard provides a overview of the status of all workers in the NativeLink clusters.
It displays information such as status, information, and metrics of each worker.

#### Key Capabilities:

- View the information of all workers.
  - Worker ID, Live Metrics(CPU, Memory, Disk, Network), Assigned Actions, Last Heartbeat
  - Status: Idle, Working, Unavailable, Offline
    - Idle: The worker is online and waiting for tasks.
    - Working: The worker is currently processing tasks. Also has sub-statuses: Downloading, Executing, Uploading
    - Unavailable: Temporarily unable to connect. Maybe worker is dead?
    - Offline: The worker is offline and not available for tasks.
  - Shows the assigned actions to each worker by tree-view.
  - Search for specific workers using keywords.

### Storage Status Overview

This overview provides a summary of the status of all storage connected to NativeLink.
It displays information such as status, configuration, and usage of each storage.

### Client Connection Overview

This overview provides a summary of the status of all clients connected to NativeLink.
This helps to identify which clients are interacting with the nativelink cluster, what they're doing.

#### Key Capabilities:
Client ID, Connection Info, Status(Idle, Disconnected), Metrics(total execution, data transferred, last seen), Activities

### Log stream viewer

This feature provides a real-time stream of logs from NativeLink components.
It allows of users/devs to monitor logs and deep-dive debugging without requiring terminal access of nativelink.
The UI should be similar as Chromium's console tab viewer.
