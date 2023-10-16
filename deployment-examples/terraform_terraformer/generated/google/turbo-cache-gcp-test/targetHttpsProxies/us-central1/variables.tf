data "terraform_remote_state" "urlMaps" {
  backend = "local"

  config = {
    path = "../../../../../generated/google/turbo-cache-gcp-test/urlMaps/us-central1/terraform.tfstate"
  }
}
