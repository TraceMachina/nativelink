  # Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

  # -- Begin Base GCI ---

  resource "google_compute_instance" "build_turbo_cache_instance" {
  for_each = {
    arm = {
      "machine_type" : var.build_arm_machine_type,
      "image" : var.build_base_image_arm,
    }
    x86 = {
      "machine_type" : var.build_x86_machine_type,
      "image" : var.build_base_image_x86,
    }
  }

  name         = "build-j-b-instance-${each.key}"
  machine_type = each.value["machine_type"]
  zone         = "us-central1-a"

  boot_disk {
    initialize_params {
      image = each.value["image"]
    }
  }

  network_interface {
    network = "default"
    access_config {
      // Ephemeral IP
    }
  }

  metadata = {
    ssh-keys = "user:${file(var.ssh_public_key_path)}"
  }

  service_account {
    scopes = ["cloud-platform"]
  }

  connection {
    host        = google_compute_instance.build_turbo_cache_instance[each.key].network_interface[0].access_config[0].nat_ip
    type        = "ssh"
    user        = "user"
    private_key = file(var.private_key_path)
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
          `# When the instance first starts up AWS may have not finished add the certs to the` &&
          `# apt servers, so we sleep for a few seconds` &&
          sleep 5 &&
          sudo DEBIAN_FRONTEND=noninteractive apt-get update &&
          sudo DEBIAN_FRONTEND=noninteractive apt-get install -y jq libssl-dev &&
          sudo mv ~/create_filesystem.sh /root/create_filesystem.sh &&
          sudo chmod +x /root/create_filesystem.sh &&
          sudo /root/create_filesystem.sh /mnt/data &&
          sudo rm -rf /tmp/* &&
          sudo mkdir -p /mnt/data/tmp &&
          sudo chmod 777 /mnt/data/tmp &&
          sudo mount --bind /mnt/data/tmp /tmp &&
          sudo chmod 777 /tmp &&
          sudo mkdir -p /mnt/data/docker &&
          sudo mkdir -p /var/lib/docker &&
          sudo mount --bind /mnt/data/docker /var/lib/docker
        EOT
      ]
    }

    provisioner "file" {
      source      = "./.terraform-turbo-cache-builder/file.tar.gz"
      destination = "/tmp/file.tar.gz"
    }

    provisioner "remote-exec" {
      inline = [
        <<EOT
          set -eux &&
          mkdir -p /tmp/turbo-cache &&
          cd /tmp/turbo-cache &&
          tar xvf /tmp/file.tar.gz &&
          sudo DEBIAN_FRONTEND=noninteractive apt-get install -y docker.io awscli &&
          cd /tmp/turbo-cache &&
          . /etc/lsb-release &&
          sudo docker build --build-arg OS_VERSION=$DISTRIB_RELEASE -t turbo-cache-runner -f ./deployment-examples/docker-compose/Dockerfile . &&
          container_id=$(sudo docker create turbo-cache-runner) &&
          `# Copy the compiled binary out of the container` &&
          sudo docker cp $container_id:/usr/local/bin/turbo-cache /usr/local/bin/turbo-cache &&
          `# Stop and remove all containers, as they are not needed` &&
          sudo docker rm $(sudo docker ps -a -q) &&
          sudo docker rmi $(sudo docker images -q) &&
          `` &&
          sudo mv /tmp/turbo-cache/deployment-examples/terraform/scripts/scheduler.json /root/scheduler.json &&
          sudo mv /tmp/turbo-cache/deployment-examples/terraform/scripts/cas.json /root/cas.json &&
          sudo mv /tmp/turbo-cache/deployment-examples/terraform/scripts/worker.json /root/worker.json &&
          sudo mv /tmp/turbo-cache/deployment-examples/terraform/scripts/start_turbo_cache.sh /root/start_turbo_cache.sh &&
          sudo chmod +x /root/start_turbo_cache.sh &&
          sudo mv /tmp/turbo-cache/deployment-examples/terraform/scripts/turbo-cache.service /etc/systemd/system/turbo-cache.service &&
          sudo systemctl enable turbo-cache &&
          sync
        EOT
      ]
    }
  }

resource "google_compute_image" "base_image" {
  for_each = {
    arm = "arm",
    x86 = "x86"
  }

  name        = "turbo_cache_${each.key}_base"
  source_disk = google_compute_instance.build_turbo_cache_instance[each.key].self_link
}
  # -- Begin Base GCI ---
