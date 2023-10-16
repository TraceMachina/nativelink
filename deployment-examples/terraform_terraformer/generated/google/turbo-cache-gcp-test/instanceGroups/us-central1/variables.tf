data "terraform_remote_state" "instanceTemplates" {
  backend = "local"

  config = {
    path = "../../../../../generated/google/turbo-cache-gcp-test/instanceTemplates/us-central1/terraform.tfstate"
  }
}
