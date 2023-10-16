data "terraform_remote_state" "networks" {
  backend = "local"

  config = {
    path = "../../../../../generated/google/turbo-cache-gcp-test/networks/us-central1/terraform.tfstate"
  }
}
