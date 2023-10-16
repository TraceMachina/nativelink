data "terraform_remote_state" "targetHttpProxies" {
  backend = "local"

  config = {
    path = "../../../../../generated/google/turbo-cache-gcp-test/targetHttpProxies/us-central1/terraform.tfstate"
  }
}

data "terraform_remote_state" "targetHttpsProxies" {
  backend = "local"

  config = {
    path = "../../../../../generated/google/turbo-cache-gcp-test/targetHttpsProxies/us-central1/terraform.tfstate"
  }
}

data "terraform_remote_state" "targetSslProxies" {
  backend = "local"

  config = {
    path = "../../../../../generated/google/turbo-cache-gcp-test/targetSslProxies/us-central1/terraform.tfstate"
  }
}
