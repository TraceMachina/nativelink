resource "google_compute_disk" "tfer--us-central1-c-002F-turbo-cache-x86-cpu-worker-instance-group-5520-1" {
  name                      = "turbo-cache-x86-cpu-worker-instance-group-5520-1"
  physical_block_size_bytes = "4096"
  project                   = "turbo-cache-gcp-test"
  provisioned_iops          = "0"
  size                      = "100"
  type                      = "pd-standard"
  zone                      = "us-central1-c"
}
