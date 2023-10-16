data "terraform_remote_state" "backendServices" {
  backend = "local"

  config = {
    path = "../../../../../generated/google/turbo-cache-gcp-test/backendServices/us-central1/terraform.tfstate"
  }
}

data "terraform_remote_state" "regionBackendServices" {
  backend = "local"

  config = {
    path = "../../../../../generated/google/turbo-cache-gcp-test/regionBackendServices/us-central1/terraform.tfstate"
  }
}
