data "terraform_remote_state" "healthChecks" {
  backend = "local"

  config = {
    path = "../../../../../generated/google/turbo-cache-gcp-test/healthChecks/us-central1/terraform.tfstate"
  }
}

data "terraform_remote_state" "instanceGroupManagers" {
  backend = "local"

  config = {
    path = "../../../../../generated/google/turbo-cache-gcp-test/instanceGroupManagers/us-central1/terraform.tfstate"
  }
}

data "terraform_remote_state" "regionInstanceGroupManagers" {
  backend = "local"

  config = {
    path = "../../../../../generated/google/turbo-cache-gcp-test/regionInstanceGroupManagers/us-central1/terraform.tfstate"
  }
}
