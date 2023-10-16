# Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

resource "google_compute_instance" "build_instance" {
  # project      = 
  name         = "turbo-cache-build-instance"
  machine_type = "e2-highcpu-16"
  zone         = var.gcp_zone

  boot_disk {
    initialize_params {
      image = "ubuntu-os-cloud/ubuntu-2004-lts"
      size = "50"
    }
  }

  network_interface {
    # network     = google_compute_network.vpc_network.self_link
    network = "default"

    access_config {
      # Ephemeral
    }
  }

  # vpc_security_group_ids = [
  #   aws_security_group.allow_ssh_sg.id,
  #   aws_security_group.ami_builder_instance_sg.id,
  #   aws_security_group.allow_aws_ec2_and_s3_endpoints.id,
  # ]

  # root_block_device {
  #   volume_size = 8
  #   volume_type = "gp3"
  # }

  # tags = {
  #   "turbo_cache:instance_type" = "ami_builder",
  # }

  metadata = {
    ssh-keys = "ubuntu:${data.tls_public_key.turbo_cache_pem.public_key_openssh}"
  }

  connection {
    host        = coalesce(self.network_interface.0.access_config.0.nat_ip, self.network_interface.0.network_ip)
    agent       = true
    type        = "ssh"
    user        = "ubuntu"
    private_key = data.tls_public_key.turbo_cache_pem.private_key_openssh
  }

  provisioner "local-exec" {
    command = <<EOT
      set -ex
      SELF_DIR=$(pwd)
      cd ../../
      rm -rf $SELF_DIR/.terraform-turbo-cache-builder
      mkdir -p $SELF_DIR/.terraform-turbo-cache-builder
      find . ! -ipath '*/target*' -and ! \( -ipath '*/.*' -and ! -name '.rustfmt.toml' -and ! -name '.bazelrc' \) -and ! -ipath './bazel-*' -type f -print0 | tar cvf $SELF_DIR/.terraform-turbo-cache-builder/file.tar.gz --null -T -
    EOT
  }

  provisioner "file" {
    source      = "./scripts/create_filesystem.sh"
    destination = "create_filesystem.sh"
  }

  # provisioner "remote-exec" {
  #   # Install and setup zfs.
  #   inline = [
  #     <<EOT
  #       set -eux &&
  #       `# When the instance first starts up AWS may have not finished add the certs to the` &&
  #       `# apt servers, so we sleep for a few seconds` &&
  #       sleep 5 &&
  #       sudo DEBIAN_FRONTEND=noninteractive apt-get update &&
  #       sudo DEBIAN_FRONTEND=noninteractive apt-get install -y build-essential autoconf automake libtool gawk alien fakeroot dkms libblkid-dev uuid-dev libudev-dev libssl-dev zlib1g-dev libaio-dev libattr1-dev libelf-dev linux-headers-generic python3 python3-dev python3-setuptools python3-cffi libffi-dev python3-packaging git libcurl4-openssl-dev debhelper-compat dh-python po-debconf python3-all-dev python3-sphinx &&
  #       cd /tmp &&
  #       wget https://github.com/openzfs/zfs/archive/refs/tags/zfs-2.1.13.tar.gz &&
  #       test $(sha256sum zfs-2.1.13.tar.gz | head -c64) == "d065719e4aefbc0513a6e652e4d15ba15b0bbfa4916442d5c99f36dd96ba0407" &&
  #       tar xzf zfs-2.1.13.tar.gz &&
  #       rm -rf zfs-2.1.13.tar.gz &&
  #       cd zfs-zfs-2.1.13/ &&
  #       sh autogen.sh &&
  #       ./configure &&
  #       make -s deb -j$(nproc) &&
  #       dpkg -i libzfs5_2.1.13-1_amd64.deb &&
  #       dpkg -i libzpool5_2.1.13-1_amd64.deb &&
  #       dpkg -i libnvpair3_2.1.13-1_amd64.deb &&
  #       dpkg -i libuutil3_2.1.13-1_amd64.deb &&
  #       dpkg -i kmod-zfs-$(uname -r)_2.1.13-1_amd64.deb &&
  #       dpkg -i zfs_2.1.13-1_amd64.deb &&
  #       modprobe zfs &&
  #       cd / &&
  #       sudo DEBIAN_FRONTEND=noninteractive apt remove -y build-essential autoconf automake libtool gawk alien fakeroot dkms libblkid-dev uuid-dev libudev-dev libssl-dev zlib1g-dev libaio-dev libattr1-dev libelf-dev linux-headers-generic python3 python3-dev python3-setuptools python3-cffi libffi-dev python3-packaging git libcurl4-openssl-dev debhelper-compat dh-python po-debconf python3-all-dev python3-sphinx &&
  #       rm -rf /tmp/zfs-zfs-2.1.13
  #     EOT
  #   ]
  # }

  provisioner "remote-exec" {
    # By moving common temp folder locations to the nvme drives (if available)
    # will greatly reduce the amount of data on the EBS volume. This also will
    # make the AMI/EBS snapshot much faster to create, since the blocks on the
    # EBS drives was not changed.
    # When the instance starts we need to give a tiny bit of time for amazon
    # to install the keys for all the apt packages.
    inline = [
      <<EOT
        set -eux &&
        sudo DEBIAN_FRONTEND=noninteractive apt-get update &&
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y curl jq build-essential lld pkg-config libssl-dev &&
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y &&
        sudo mv ~/create_filesystem.sh /root/create_filesystem.sh &&
        sudo chmod +x /root/create_filesystem.sh &&
        sudo /root/create_filesystem.sh /mnt/data &&
        sudo rm -rf /home/ubuntu/*
      EOT
    ]
  }

  provisioner "file" {
    source      = "./.terraform-turbo-cache-builder/file.tar.gz"
    destination = "/tmp/file.tar.gz"
  }

  # provisioner "remote-exec" {
  #   inline = [
  #     <<EOT
  #       set -eux &&
  #       echo "AWS_SECRET_ACCESS_KEY=${google_storage_hmac_key.key.secret}" | sudo tee /root/.gcp_secrets &&
  #       echo "AWS_ACCESS_KEY_ID=${google_storage_hmac_key.key.access_id}" | sudo tee -a /root/.gcp_secrets &&
  #       sudo chmod 0400 /root/.gcp_secrets
  #     EOT
  #   ]
  # }

  provisioner "remote-exec" {
    inline = [
      <<EOT
        set -eux &&
        mkdir -p /home/ubuntu/turbo-cache &&
        cd /home/ubuntu/turbo-cache &&
        tar xvf /tmp/file.tar.gz &&
        . ~/.cargo/env &&
        cargo build --release --bin cas &&
        sudo ln -s /home/ubuntu/turbo-cache/target/release/cas /usr/local/bin/turbo-cache &&
        `` &&
        sudo mv /home/ubuntu/turbo-cache/deployment-examples/terraform_gcp/scripts/scheduler.json /root/scheduler.json &&
        sudo mv /home/ubuntu/turbo-cache/deployment-examples/terraform_gcp/scripts/cas.json /root/cas.json &&
        sudo mv /home/ubuntu/turbo-cache/deployment-examples/terraform_gcp/scripts/worker.json /root/worker.json &&
        sudo mv /home/ubuntu/turbo-cache/deployment-examples/terraform_gcp/scripts/start_turbo_cache.sh /root/start_turbo_cache.sh &&
        sudo mv /home/ubuntu/turbo-cache/deployment-examples/terraform_gcp/scripts/entrypoint.sh /root/entrypoint.sh &&
        sudo chmod +x /root/start_turbo_cache.sh &&
        sudo mv /home/ubuntu/turbo-cache/deployment-examples/terraform_gcp/scripts/turbo-cache.service /etc/systemd/system/turbo-cache.service &&
        sudo systemctl enable turbo-cache &&
        sync
      EOT
    ]
  }

  provisioner "remote-exec" {
    inline = [
      <<EOT
        set -eux &&
        echo "deb [signed-by=/usr/share/keyrings/cloud.google.gpg] https://packages.cloud.google.com/apt cloud-sdk main" | sudo tee -a /etc/apt/sources.list.d/google-cloud-sdk.list &&
        curl https://packages.cloud.google.com/apt/doc/apt-key.gpg | sudo apt-key --keyring /usr/share/keyrings/cloud.google.gpg add - &&
        sudo DEBIAN_FRONTEND=noninteractive apt-get update &&
        sudo DEBIAN_FRONTEND=noninteractive apt-get install google-cloud-cli
      EOT
    ]
  }

# sudo apt-get purge -y nvidia-docker2
# curl -s -L https://nvidia.github.io/nvidia-docker/gpgkey | \
#     sudo apt-key add -
# distribution=$(. /etc/os-release;echo $ID$VERSION_ID)
# curl -s -L https://nvidia.github.io/nvidia-docker/$distribution/nvidia-docker.list | \
#     sudo tee /etc/apt/sources.list.d/nvidia-docker.list
# sudo apt-get update
# # Install nvidia-docker2 and reload the Docker daemon configuration
# sudo apt-get install -y nvidia-docker2

  provisioner "remote-exec" {
    inline = [
      <<EOT
        set -eux &&
        curl -s -L https://nvidia.github.io/nvidia-docker/gpgkey | sudo apt-key add - &&
        curl -s -L https://nvidia.github.io/nvidia-docker/$(. /etc/os-release;echo $ID$VERSION_ID)/nvidia-docker.list |
            sudo tee /etc/apt/sources.list.d/nvidia-docker.list &&
        sudo DEBIAN_FRONTEND=noninteractive apt-get update &&
        `# Install nvidia-docker2 and reload the Docker daemon configuration` &&
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y nvidia-docker2
      EOT
    ]
  }

  # provisioner "remote-exec" {
  #   inline = [
  #     <<EOT
  #       set -eux &&
  #       mkdir -p /tmp/turbo-cache &&
  #       cd /tmp/turbo-cache &&
  #       tar xvf /tmp/file.tar.gz &&
  #       sudo DEBIAN_FRONTEND=noninteractive apt-get install -y docker.io &&
  #       cd /tmp/turbo-cache &&
  #       . /etc/lsb-release &&
  #       sudo docker build --build-arg OS_VERSION=$DISTRIB_RELEASE -t turbo-cache-runner -f ./deployment-examples/docker-compose/Dockerfile . &&
  #       container_id=$(sudo docker create turbo-cache-runner) &&
  #       `# Copy the compiled binary out of the container` &&
  #       sudo docker cp $container_id:/usr/local/bin/turbo-cache /usr/local/bin/turbo-cache &&
  #       `# Stop and remove all containers, as they are not needed` &&
  #       sudo docker rm $(sudo docker ps -a -q) &&
  #       sudo docker rmi $(sudo docker images -q) &&
  #       `` &&
  #       sudo mv /tmp/turbo-cache/deployment-examples/terraform_gcp/scripts/scheduler.json /root/scheduler.json &&
  #       sudo mv /tmp/turbo-cache/deployment-examples/terraform_gcp/scripts/cas.json /root/cas.json &&
  #       sudo mv /tmp/turbo-cache/deployment-examples/terraform_gcp/scripts/worker.json /root/worker.json &&
  #       sudo mv /tmp/turbo-cache/deployment-examples/terraform_gcp/scripts/start_turbo_cache.sh /root/start_turbo_cache.sh &&
  #       sudo chmod +x /root/start_turbo_cache.sh &&
  #       sudo mv /tmp/turbo-cache/deployment-examples/terraform_gcp/scripts/turbo-cache.service /etc/systemd/system/turbo-cache.service &&
  #       sudo systemctl enable turbo-cache &&
  #       sync
  #     EOT
  #   ]
  # }
}

resource "google_compute_snapshot" "base_snapshot" {
  name              = "turbo-cache-base-snapshot"
  source_disk       = google_compute_instance.build_instance.boot_disk.0.source
  zone              = var.gcp_zone
  storage_locations = [var.gcp_region]
}

resource "google_compute_image" "base_image" {
  name              = "turbo-cache-base-image"
  source_snapshot   = google_compute_snapshot.base_snapshot.self_link
  storage_locations = [var.gcp_region]
}
