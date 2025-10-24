#!/bin/bash
# Copyright 2024 The NativeLink Authors. All rights reserved.
#
# Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    See LICENSE file for details
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# NativeLink JuiceFS Multi-Worker Setup Script
# This script sets up a complete NativeLink deployment with JuiceFS shared storage

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
COMPOSE_FILE="docker-compose-juicefs.yml"
WORKER_COUNT=3
JUICEFS_MOUNT_DIR="/tmp/juicefs-mount"

echo -e "${GREEN}==================================${NC}"
echo -e "${GREEN}NativeLink JuiceFS Setup${NC}"
echo -e "${GREEN}==================================${NC}"
echo ""

# Check prerequisites
echo -e "${YELLOW}Checking prerequisites...${NC}"

if ! command -v docker &> /dev/null; then
    echo -e "${RED}Error: Docker is not installed${NC}"
    echo "Please install Docker: https://docs.docker.com/get-docker/"
    exit 1
fi

if ! command -v docker-compose &> /dev/null && ! docker compose version &> /dev/null; then
    echo -e "${RED}Error: Docker Compose is not installed${NC}"
    echo "Please install Docker Compose: https://docs.docker.com/compose/install/"
    exit 1
fi

# Detect which docker compose command to use
if command -v docker-compose &> /dev/null; then
    DOCKER_COMPOSE="docker-compose"
else
    DOCKER_COMPOSE="docker compose"
fi

echo -e "${GREEN}✓ Docker installed${NC}"
echo -e "${GREEN}✓ Docker Compose installed (using: $DOCKER_COMPOSE)${NC}"
echo ""

# Create JuiceFS mount directory
echo -e "${YELLOW}Creating JuiceFS mount directory...${NC}"
mkdir -p "$JUICEFS_MOUNT_DIR"
echo -e "${GREEN}✓ Created $JUICEFS_MOUNT_DIR${NC}"
echo ""

# Build NativeLink image
echo -e "${YELLOW}Building NativeLink image...${NC}"
if $DOCKER_COMPOSE -f "$COMPOSE_FILE" build; then
    echo -e "${GREEN}✓ NativeLink image built successfully${NC}"
else
    echo -e "${RED}✗ Failed to build NativeLink image${NC}"
    exit 1
fi
echo ""

# Start services
echo -e "${YELLOW}Starting services...${NC}"
echo "  - Redis (JuiceFS metadata)"
echo "  - MinIO (Object storage)"
echo "  - JuiceFS mount"
echo "  - CAS server"
echo "  - Scheduler"
echo "  - Workers (x${WORKER_COUNT})"
echo ""

if $DOCKER_COMPOSE -f "$COMPOSE_FILE" up -d; then
    echo -e "${GREEN}✓ All services started${NC}"
else
    echo -e "${RED}✗ Failed to start services${NC}"
    exit 1
fi
echo ""

# Wait for services to be healthy
echo -e "${YELLOW}Waiting for services to be healthy...${NC}"

wait_for_service() {
    local service=$1
    local max_attempts=30
    local attempt=0

    while [ $attempt -lt $max_attempts ]; do
        if $DOCKER_COMPOSE -f "$COMPOSE_FILE" ps "$service" | grep -q "healthy"; then
            echo -e "${GREEN}✓ $service is healthy${NC}"
            return 0
        fi
        attempt=$((attempt + 1))
        echo -n "."
        sleep 2
    done

    echo -e "${RED}✗ $service did not become healthy${NC}"
    return 1
}

echo -n "Redis: "
wait_for_service redis

echo -n "MinIO: "
wait_for_service minio

echo -n "JuiceFS: "
wait_for_service juicefs-mount

echo -n "CAS Server: "
wait_for_service cas-server

echo -n "Scheduler: "
wait_for_service scheduler

echo ""

# Verify JuiceFS mount
echo -e "${YELLOW}Verifying JuiceFS mount...${NC}"
if docker exec nativelink-juicefs-mount-1 sh -c "[ -d /mnt/jfs/.stats ]" 2> /dev/null; then
    echo -e "${GREEN}✓ JuiceFS mounted successfully${NC}"
else
    echo -e "${RED}✗ JuiceFS mount verification failed${NC}"
    echo "Check logs: $DOCKER_COMPOSE -f $COMPOSE_FILE logs juicefs-mount"
    exit 1
fi
echo ""

# Show JuiceFS status
echo -e "${YELLOW}JuiceFS Status:${NC}"
docker exec nativelink-juicefs-mount-1 juicefs status redis://redis:6379/1
echo ""

# Check worker status
echo -e "${YELLOW}Checking workers...${NC}"
for i in $(seq 1 $WORKER_COUNT); do
    if $DOCKER_COMPOSE -f "$COMPOSE_FILE" ps "worker-$i" | grep -q "Up"; then
        echo -e "${GREEN}✓ worker-$i is running${NC}"
    else
        echo -e "${RED}✗ worker-$i is not running${NC}"
    fi
done
echo ""

# Display service endpoints
echo -e "${GREEN}==================================${NC}"
echo -e "${GREEN}Deployment Complete!${NC}"
echo -e "${GREEN}==================================${NC}"
echo ""
echo "Service Endpoints:"
echo "  - CAS Server (gRPC):     grpc://localhost:50051"
echo "  - Scheduler (gRPC):      grpc://localhost:50052"
echo "  - Worker API (gRPC):     grpc://localhost:50061"
echo "  - MinIO Console:         http://localhost:9001"
echo "  - Redis:                 redis://localhost:6379"
echo ""
echo "Useful Commands:"
echo "  - View logs:             $DOCKER_COMPOSE -f $COMPOSE_FILE logs -f"
echo "  - View worker logs:      $DOCKER_COMPOSE -f $COMPOSE_FILE logs -f worker-1"
echo "  - Check JuiceFS status:  docker exec nativelink-juicefs-mount-1 juicefs status redis://redis:6379/1"
echo "  - Scale workers:         $DOCKER_COMPOSE -f $COMPOSE_FILE up -d --scale worker-1=5"
echo "  - Stop all services:     $DOCKER_COMPOSE -f $COMPOSE_FILE down"
echo "  - Clean volumes:         $DOCKER_COMPOSE -f $COMPOSE_FILE down -v"
echo ""
echo "Test the setup with Bazel:"
cat << 'EOF'
  bazel build \
    --remote_cache=grpc://localhost:50051 \
    --remote_executor=grpc://localhost:50052 \
    //your:target
EOF
echo ""
echo -e "${GREEN}Happy building! 🚀${NC}"
