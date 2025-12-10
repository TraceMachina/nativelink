<!--
Copyright 2024 The NativeLink Authors. All rights reserved.

Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

   See LICENSE file for details

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
-->

# JuiceFS Multi-Worker Deployment for NativeLink

This directory contains a production-ready Docker Compose configuration for running NativeLink with multiple workers backed by JuiceFS distributed storage.

## What's Included

- **Redis**: JuiceFS metadata engine
- **MinIO**: S3-compatible object storage for JuiceFS data
- **JuiceFS Mount**: Dedicated container maintaining the FUSE filesystem
- **CAS Server**: NativeLink Content Addressable Storage server
- **Scheduler**: Work distribution coordinator
- **Workers (3)**: Build execution workers with shared CAS via JuiceFS

## Architecture

```
                    ┌─────────────────┐
                    │  Bazel Client   │
                    └────────┬────────┘
                             │
            ┌────────────────┼────────────────┐
            │                │                │
      ┌─────▼─────┐    ┌────▼────┐    ┌─────▼─────┐
      │  Worker 1 │    │Worker 2 │    │  Worker 3 │
      └─────┬─────┘    └────┬────┘    └─────┬─────┘
            │               │               │
            └───────────────┼───────────────┘
                            │
                     ┌──────▼──────┐
                     │  Scheduler  │
                     └──────┬──────┘
                            │
                     ┌──────▼──────┐
                     │ CAS Server  │
                     └─────────────┘

    All workers share CAS via JuiceFS:

      ┌──────┬──────┬──────┐
      │  W1  │  W2  │  W3  │
      └───┬──┴───┬──┴───┬──┘
          │      │      │
          └──────┼──────┘
                 │
          ┌──────▼──────┐
          │   JuiceFS   │
          │  Filesystem │
          └──────┬──────┘
                 │
      ┌──────────┴──────────┐
      │                     │
┌─────▼─────┐      ┌────────▼────────┐
│   Redis   │      │     MinIO       │
│ (Metadata)│      │ (Object Store)  │
└───────────┘      └─────────────────┘
```

## Quick Start

### Option 1: Automated Setup (Recommended)

```bash
# Run the setup script
./setup-juicefs.sh

# The script will:
# - Check prerequisites
# - Build NativeLink image
# - Start all services
# - Verify JuiceFS mount
# - Display status and commands
```

### Option 2: Manual Setup

```bash
# Build the NativeLink image
docker-compose -f docker-compose-juicefs.yml build

# Start all services
docker-compose -f docker-compose-juicefs.yml up -d

# Check service status
docker-compose -f docker-compose-juicefs.yml ps

# View logs
docker-compose -f docker-compose-juicefs.yml logs -f
```

## Configuration Files

- **`docker-compose-juicefs.yml`**: Main Docker Compose configuration
- **`worker-juicefs.json5`**: Worker configuration using JuiceFS-backed storage
- **`cas-server-multi-worker.json5`**: CAS server configuration
- **`scheduler-multi-worker.json5`**: Scheduler configuration

## Verifying the Setup

### 1. Check Service Health

```bash
# All services should show "healthy" or "running"
docker-compose -f docker-compose-juicefs.yml ps
```

Expected output:
```
NAME                            STATUS
nativelink-cas-server-1         Up (healthy)
nativelink-juicefs-mount-1      Up (healthy)
nativelink-minio-1              Up (healthy)
nativelink-redis-1              Up (healthy)
nativelink-scheduler-1          Up (healthy)
nativelink-worker-1-1           Up
nativelink-worker-2-1           Up
nativelink-worker-3-1           Up
```

### 2. Verify JuiceFS Mount

```bash
# Check JuiceFS status
docker exec nativelink-juicefs-mount-1 juicefs status redis://redis:6379/1

# Check filesystem
docker exec nativelink-juicefs-mount-1 df -h /mnt/jfs

# Verify .stats directory (JuiceFS health indicator)
docker exec nativelink-juicefs-mount-1 ls -la /mnt/jfs/.stats/
```

### 3. Test Worker Access to JuiceFS

```bash
# Exec into a worker
docker exec -it nativelink-worker-1-1 sh

# Inside the worker container:
ls -la /mnt/jfs/cas/
touch /mnt/jfs/cas/test-file
ls -la /mnt/jfs/cas/test-file

# Exit the worker
exit

# Verify from another worker (should see the same file)
docker exec nativelink-worker-2-1 ls -la /mnt/jfs/cas/test-file
```

### 4. Test Build Execution

```bash
# In your Bazel workspace, run:
bazel build \
  --remote_cache=grpc://localhost:50051 \
  --remote_executor=grpc://localhost:50052 \
  //your:target

# Check worker logs to see which worker executed the action
docker-compose -f docker-compose-juicefs.yml logs worker-1 worker-2 worker-3 | grep "Executing action"
```

## Scaling Workers

### Add More Workers

Edit `docker-compose-juicefs.yml` and add:

```yaml
worker-4:
  image: nativelink:latest
  privileged: true
  cap_add:
    - SYS_ADMIN
  devices:
    - /dev/fuse
  volumes:
    - type: volume
      source: juicefs-mount
      target: /mnt/jfs
      volume:
        nocopy: true
    - worker4-work:/tmp/worker4
    - ./worker-juicefs.json5:/nativelink-config.json5
  environment:
    - RUST_LOG=info,nativelink=debug
    - SCHEDULER_ENDPOINT=scheduler
    - CAS_ENDPOINT=cas-server
    - WORKER_NAME=worker-4
    - WORKER_ID=4
  command: nativelink /nativelink-config.json5
  depends_on:
    scheduler:
      condition: service_healthy
    juicefs-mount:
      condition: service_healthy
  networks:
    - nativelink-network
```

Don't forget to add the volume:
```yaml
volumes:
  worker4-work:
    driver: local
```

Then restart:
```bash
docker-compose -f docker-compose-juicefs.yml up -d
```

## Monitoring

### View Logs

```bash
# All services
docker-compose -f docker-compose-juicefs.yml logs -f

# Specific service
docker-compose -f docker-compose-juicefs.yml logs -f worker-1

# JuiceFS mount
docker-compose -f docker-compose-juicefs.yml logs -f juicefs-mount
```

### JuiceFS Statistics

```bash
# Real-time stats
docker exec nativelink-juicefs-mount-1 juicefs stats /mnt/jfs

# Detailed status
docker exec nativelink-juicefs-mount-1 juicefs status redis://redis:6379/1
```

### Resource Usage

```bash
# Container resource usage
docker stats

# JuiceFS cache usage
docker exec nativelink-juicefs-mount-1 du -sh /var/jfsCache
```

### MinIO Console

Access MinIO web console:
```
http://localhost:9001
Username: minioadmin
Password: minioadmin
```

## Troubleshooting

### JuiceFS Mount Not Working

```bash
# Check JuiceFS logs
docker-compose -f docker-compose-juicefs.yml logs juicefs-mount

# Verify Redis is accessible
docker exec nativelink-juicefs-mount-1 redis-cli -h redis ping

# Check FUSE device
docker exec nativelink-juicefs-mount-1 ls -la /dev/fuse
```

### Workers Can't Access JuiceFS

```bash
# Check if volume is properly mounted
docker exec nativelink-worker-1-1 df -h | grep jfs

# Verify permissions
docker exec nativelink-worker-1-1 ls -la /mnt/jfs/

# Check for FUSE capability
docker exec nativelink-worker-1-1 cat /proc/filesystems | grep fuse
```

### Slow Performance

```bash
# Check JuiceFS cache hit rate
docker exec nativelink-juicefs-mount-1 juicefs stats /mnt/jfs

# Increase cache size in docker-compose-juicefs.yml:
# --cache-size 20480  # 20GB

# Increase max-uploads:
# --max-uploads 50

# Then recreate the juicefs-mount container:
docker-compose -f docker-compose-juicefs.yml up -d --force-recreate juicefs-mount
```

### Out of Space

```bash
# Check available space
docker exec nativelink-juicefs-mount-1 df -h /mnt/jfs

# Check cache usage
docker exec nativelink-juicefs-mount-1 du -sh /var/jfsCache

# Clean up old cache (JuiceFS does this automatically based on --cache-size)
```

## Cleanup

### Stop Services (Keep Data)

```bash
docker-compose -f docker-compose-juicefs.yml down
```

### Stop Services and Delete Volumes

```bash
docker-compose -f docker-compose-juicefs.yml down -v
```

### Complete Cleanup

```bash
# Stop and remove everything
docker-compose -f docker-compose-juicefs.yml down -v --remove-orphans

# Remove NativeLink image
docker rmi nativelink:latest

# Clean up JuiceFS mount directory
rm -rf /tmp/juicefs-mount
```

## Production Considerations

### Use External Services

For production deployments, replace MinIO and Redis with managed services:

1. **Replace MinIO with AWS S3**:
   - Update `juicefs format` command in docker-compose-juicefs.yml
   - Use S3 bucket URL instead of MinIO

2. **Replace Redis with managed Redis**:
   - AWS ElastiCache, Google Cloud Memory Store, or Azure Cache
   - Update metadata URL in format and mount commands

3. **Use PostgreSQL for metadata**:
   - More reliable for large-scale deployments
   - Better for billions of files

### Security Hardening

1. **Enable TLS**:
   ```bash
   juicefs mount rediss://redis:6379/1 /mnt/jfs  # Note: rediss
   ```

2. **Encrypt data at rest**:
   ```bash
   juicefs format --encrypt-rsa-key /path/to/key.pem ...
   ```

3. **Use secrets management**:
   - Don't hard code credentials
   - Use Docker secrets or Kubernetes secrets

### High Availability

1. **Redis HA**:
   - Use Redis Sentinel or Redis Cluster
   - Multiple Redis instances with automatic failover

2. **Multiple JuiceFS Mounts**:
   - Each worker can have its own mount
   - Automatic metadata cache synchronization

3. **Object Storage Redundancy**:
   - S3 with cross-region replication
   - MinIO with erasure coding

## Next Steps

- **Production Deployment**: See [juicefs-integration.md](../docs/juicefs-integration.md) for Kubernetes deployment
- **Performance Tuning**: Optimize JuiceFS cache and concurrency settings
- **Monitoring**: Set up Prometheus metrics collection
- **Backup**: Configure metadata and object storage backups

## Support

- **NativeLink**: [Slack](https://forms.gle/LtaWSixEC6bYi5xF7) | [GitHub](https://github.com/TraceMachina/nativelink)
- **JuiceFS**: [Docs](https://juicefs.com/docs/) | [Slack](https://juicefs.com/docs/community/slack)
