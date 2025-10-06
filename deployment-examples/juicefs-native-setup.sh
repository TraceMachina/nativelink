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

# Native JuiceFS Setup for NativeLink Multi-Worker
# This script sets up JuiceFS WITHOUT Docker

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}NativeLink JuiceFS Native Setup${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""

# Configuration
JUICEFS_MOUNT_POINT="/tmp/nativelink-jfs"

# Step 1: Install JuiceFS
echo -e "${YELLOW}Step 1: Installing JuiceFS...${NC}"
if command -v juicefs &> /dev/null; then
    echo -e "${GREEN}✓ JuiceFS already installed${NC}"
    juicefs version
else
    echo "Installing JuiceFS..."
    curl -sSL https://d.juicefs.com/install | sh -
    if command -v juicefs &> /dev/null; then
        echo -e "${GREEN}✓ JuiceFS installed successfully${NC}"
        juicefs version
    else
        echo -e "${RED}✗ Failed to install JuiceFS${NC}"
        exit 1
    fi
fi
echo ""

# Step 2: Check for Redis
echo -e "${YELLOW}Step 2: Checking Redis...${NC}"
if command -v redis-server &> /dev/null; then
    echo -e "${GREEN}✓ Redis is installed${NC}"
    # Check if Redis is running
    if redis-cli ping &> /dev/null; then
        echo -e "${GREEN}✓ Redis is running${NC}"
    else
        echo -e "${YELLOW}⚠ Redis is not running. Starting Redis...${NC}"
        echo "You can start Redis with: redis-server --daemonize yes"
        echo "Or install via: brew install redis (macOS) / apt-get install redis (Linux)"
    fi
else
    echo -e "${YELLOW}⚠ Redis not found${NC}"
    echo "Install Redis:"
    echo "  macOS: brew install redis && brew services start redis"
    echo "  Linux: sudo apt-get install redis-server && sudo systemctl start redis"
    echo ""
    echo "Or use Docker for just Redis and MinIO:"
    echo "  docker run -d --name redis -p 6379:6379 redis:7-alpine"
fi
echo ""

# Step 3: Check for MinIO (or use S3)
echo -e "${YELLOW}Step 3: Object Storage Setup...${NC}"
echo "You have two options:"
echo "  1. Use MinIO locally (requires Docker or binary)"
echo "  2. Use AWS S3 (requires AWS credentials)"
echo ""
echo "For MinIO with Docker:"
cat << 'EOF'
  docker run -d --name minio \
    -p 9000:9000 -p 9001:9001 \
    -v /tmp/minio-data:/data \
    -e MINIO_ROOT_USER=minioadmin \
    -e MINIO_ROOT_PASSWORD=minioadmin \
    minio/minio server /data --console-address ':9001'

  # Create bucket:
  docker run --rm --network host minio/mc \
    mc alias set local http://localhost:9000 minioadmin minioadmin && \
    mc mb local/nativelink-cas
EOF
echo ""

# Step 4: Format JuiceFS
echo -e "${YELLOW}Step 4: Format JuiceFS filesystem...${NC}"
echo ""
read -r -p "Enter Redis URL (default: redis://localhost:6379/1): " REDIS_URL
REDIS_URL=${REDIS_URL:-redis://localhost:6379/1}

read -r -p "Enter storage type (minio/s3) (default: minio): " STORAGE_TYPE
STORAGE_TYPE=${STORAGE_TYPE:-minio}

if [ "$STORAGE_TYPE" = "minio" ]; then
    read -r -p "Enter MinIO endpoint (default: http://localhost:9000): " MINIO_ENDPOINT
    MINIO_ENDPOINT=${MINIO_ENDPOINT:-http://localhost:9000}

    read -r -p "Enter MinIO access key (default: minioadmin): " MINIO_ACCESS_KEY
    MINIO_ACCESS_KEY=${MINIO_ACCESS_KEY:-minioadmin}

    read -r -p "Enter MinIO secret key (default: minioadmin): " MINIO_SECRET_KEY
    MINIO_SECRET_KEY=${MINIO_SECRET_KEY:-minioadmin}

    BUCKET="${MINIO_ENDPOINT}/nativelink-cas"

    echo ""
    echo "Formatting JuiceFS with MinIO backend..."
    juicefs format \
        --storage minio \
        --bucket "$BUCKET" \
        --access-key "$MINIO_ACCESS_KEY" \
        --secret-key "$MINIO_SECRET_KEY" \
        "$REDIS_URL" \
        nativelink-cas || echo -e "${YELLOW}⚠ Filesystem may already be formatted${NC}"
else
    read -r -p "Enter S3 bucket (s3://your-bucket): " S3_BUCKET
    read -r -p "Enter AWS access key: " AWS_ACCESS_KEY
    read -r -p "Enter AWS secret key: " AWS_SECRET_KEY

    echo ""
    echo "Formatting JuiceFS with S3 backend..."
    juicefs format \
        --storage s3 \
        --bucket "$S3_BUCKET" \
        --access-key "$AWS_ACCESS_KEY" \
        --secret-key "$AWS_SECRET_KEY" \
        "$REDIS_URL" \
        nativelink-cas || echo -e "${YELLOW}⚠ Filesystem may already be formatted${NC}"
fi
echo ""

# Step 5: Create mount point
echo -e "${YELLOW}Step 5: Creating mount point...${NC}"
mkdir -p "$JUICEFS_MOUNT_POINT"
mkdir -p /var/jfsCache
echo -e "${GREEN}✓ Mount point created: $JUICEFS_MOUNT_POINT${NC}"
echo ""

# Step 6: Mount JuiceFS
echo -e "${YELLOW}Step 6: Mounting JuiceFS...${NC}"
if mountpoint -q "$JUICEFS_MOUNT_POINT" 2> /dev/null; then
    echo -e "${GREEN}✓ JuiceFS already mounted at $JUICEFS_MOUNT_POINT${NC}"
else
    echo "Mounting JuiceFS with optimized settings..."
    juicefs mount \
        --cache-dir /var/jfsCache \
        --cache-size 10240 \
        --writeback \
        --max-uploads 20 \
        --max-deletes 10 \
        --buffer-size 300 \
        --prefetch 3 \
        --background \
        "$REDIS_URL" \
        "$JUICEFS_MOUNT_POINT"

    sleep 2
    if mountpoint -q "$JUICEFS_MOUNT_POINT" 2> /dev/null || [ -d "$JUICEFS_MOUNT_POINT/.stats" ]; then
        echo -e "${GREEN}✓ JuiceFS mounted successfully${NC}"
    else
        echo -e "${RED}✗ Failed to mount JuiceFS${NC}"
        exit 1
    fi
fi
echo ""

# Step 7: Create CAS directory structure
echo -e "${YELLOW}Step 7: Creating CAS directory structure...${NC}"
mkdir -p "$JUICEFS_MOUNT_POINT/cas/content"
mkdir -p "$JUICEFS_MOUNT_POINT/cas/temp"
echo -e "${GREEN}✓ Directory structure created${NC}"
echo ""

# Step 8: Display status
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}Setup Complete!${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
echo "JuiceFS Mount Point: $JUICEFS_MOUNT_POINT"
echo "JuiceFS Status:"
juicefs status "$REDIS_URL"
echo ""
echo "Directory Structure:"
ls -la "$JUICEFS_MOUNT_POINT/cas/"
echo ""
echo -e "${YELLOW}Next Steps:${NC}"
echo ""
echo "1. Update your NativeLink worker configuration to use:"
echo "   content_path: \"$JUICEFS_MOUNT_POINT/cas/content\""
echo ""
echo "2. Start NativeLink workers:"
echo "   ./nativelink /path/to/worker-config.json5"
echo ""
echo "3. Monitor JuiceFS:"
echo "   juicefs stats $JUICEFS_MOUNT_POINT"
echo ""
echo "4. Unmount when done:"
echo "   juicefs umount $JUICEFS_MOUNT_POINT"
echo ""
echo -e "${GREEN}Happy building! 🚀${NC}"
