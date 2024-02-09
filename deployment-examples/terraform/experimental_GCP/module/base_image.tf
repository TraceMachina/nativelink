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

resource "google_compute_instance" "build_instance" {
  project      = var.gcp_project_id
  provider     = google-beta
  name         = "${var.project_prefix}-build-instance"
  machine_type = var.base_image_machine_type
  zone         = var.gcp_zone

  boot_disk {
    initialize_params {
      image = "ubuntu-os-cloud/ubuntu-2204-lts"
      size  = "10"
    }
  }

  network_interface {
    network = data.google_compute_network.default.id

    access_config {
      # Ephemeral.
      network_tier = "STANDARD"
    }
  }

  scheduling {
    provisioning_model          = "SPOT"
    preemptible                 = true
    automatic_restart           = false
    instance_termination_action = "DELETE"

    # This instance is not needed, so shut it down after 1 hour.
    max_run_duration {
      seconds = 3600
    }
  }

  metadata = {
    ssh-keys = "ubuntu:${data.tls_public_key.NATIVELINK_pem.public_key_openssh}"
  }

  connection {
    host        = coalesce(self.network_interface.0.access_config.0.nat_ip, self.network_interface.0.network_ip)
    agent       = true
    type        = "ssh"
    user        = "ubuntu"
    private_key = data.tls_public_key.NATIVELINK_pem.private_key_openssh
  }

  # Create tarball of current nativelink checkout.
  # Note: In production this should be changed to some pinned release version.
  provisioner "local-exec" {
    command = <<EOT
      set -ex
      ROOT_MODULE="$(realpath ${path.root})"
      rm -rf $ROOT_MODULE/.terraform-nativelink-builder
      mkdir -p $ROOT_MODULE/.terraform-nativelink-builder
      cd $ROOT_MODULE/../../../../..
      find . ! -ipath '*/target*' -and ! \( -ipath '*/.*' -and ! -name '.rustfmt.toml' -and ! -name '.bazelrc' \) -and ! -ipath './bazel-*' -type f -print0 | tar cvf $ROOT_MODULE/.terraform-nativelink-builder/file.tar.gz --null -T -
    EOT
  }

  provisioner "file" {
    source      = "${path.module}/scripts/create_filesystem.sh"
    destination = "create_filesystem.sh"
  }

  # Prepare our instance.
  provisioner "remote-exec" {
    inline = [
      <<EOT
        set -eux
        sudo DEBIAN_FRONTEND=noninteractive apt-get update &&
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y curl jq build-essential lld pkg-config libssl-dev &&
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y &&
        sudo mv ~/create_filesystem.sh /root/create_filesystem.sh &&
        sudo chmod +x /root/create_filesystem.sh &&
        sudo /root/create_filesystem.sh /mnt/data
      EOT
    ]
  }

  # Upload our tarball to the instance.
  provisioner "file" {
    source      = "${path.root}/.terraform-nativelink-builder/file.tar.gz"
    destination = "/tmp/file.tar.gz"
  }

  # Build and install nativelink.
  provisioner "remote-exec" {
    inline = [
      <<EOT
        set -eux &&
        mkdir -p /tmp/nativelink &&
        cd /tmp/nativelink &&
        tar xvf /tmp/file.tar.gz &&
        . ~/.cargo/env &&
        cargo build --release --bin nativelink &&
        sudo mv /tmp/nativelink/target/release/nativelink /usr/local/bin/nativelink &&
        `` &&
        cd /tmp/nativelink/deployment-examples/terraform/experimental_GCP/module/scripts &&
        sudo mv ./bb_browser_config.json    /root/bb_browser_config.json &&
        sudo mv ./browser_proxy.json        /root/browser_proxy.json &&
        sudo mv ./scheduler.json            /root/scheduler.json &&
        sudo mv ./cas.json                  /root/cas.json &&
        sudo mv ./worker.json               /root/worker.json &&
        sudo mv ./start_nativelink.sh      /root/start_nativelink.sh &&
        sudo mv ./entrypoint.sh             /root/entrypoint.sh &&
        sudo mv ./cloud_publisher.py        /root/cloud_publisher.py &&
        `` &&
        sudo mv ./nativelink.service       /etc/systemd/system/nativelink.service &&
        sudo chmod +x /root/start_nativelink.sh &&
        sudo systemctl enable nativelink &&
        sudo rm -rf /tmp/file.tar.gz /tmp/nativelink &&
        sync
      EOT
    ]
  }

  # Install cloud monitoring publishing agent.
  provisioner "remote-exec" {
    inline = [
      <<EOT
        set -eux &&
        echo "deb [signed-by=/usr/share/keyrings/cloud.google.gpg] https://packages.cloud.google.com/apt cloud-sdk main" | sudo tee -a /etc/apt/sources.list.d/google-cloud-sdk.list &&
        curl https://packages.cloud.google.com/apt/doc/apt-key.gpg | sudo apt-key --keyring /usr/share/keyrings/cloud.google.gpg add - &&
        sudo DEBIAN_FRONTEND=noninteractive apt-get update &&
        sudo DEBIAN_FRONTEND=noninteractive apt-get install google-cloud-cli python3 python3-pip -y &&
        `# Scheduler needs to push metrics to cloud watch.` &&
        sudo pip3 install google-cloud-monitoring &&
        sync
      EOT
    ]
  }
}

resource "google_compute_snapshot" "base_snapshot" {
  name              = "${var.project_prefix}-base-snapshot"
  source_disk       = google_compute_instance.build_instance.boot_disk.0.source
  zone              = var.gcp_zone
  storage_locations = [var.gcp_region]
}

resource "google_compute_image" "base_image" {
  name              = "${var.project_prefix}-base-image"
  source_snapshot   = google_compute_snapshot.base_snapshot.id
  storage_locations = [var.gcp_region]
}
