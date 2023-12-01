# TODO

Documentation coming soon.

# TL;Dr
```sh

# First we need to apply the global config. This config
# is unlikely to change much. The "dev" section below
# depends on this "global" section to be applied first.
# It is done this way to reduce cost of development, since
# SSL certs costs ~$20 every time they are generated, so we
# generate them only once and keep using the same one.
#
# Important: Once it is applied, you need to immediately
# create a "NS" record to the domain specified in "gcp_dns_zone"
# in the whatever DNS service you are using and point it to the
# NS record specified by the GCP DNS zone it created.
cd deployment-examples/terraform/GCP/deployments/global

terraform init
terraform apply \
  -var gcp_project_id=project-name-goes-here \
  -var gcp_dns_zone=my-domain.example.com \
  -var gcp_region=us-central1 \
  -var gcp_zone=us-central1-a

# After "global" is applied we need to apply the "dev" section.
# This is the majority of the configuration.
cd deployment-examples/terraform/GCP/deployments/dev

terraform init
terraform apply \
  -var gcp_project_id=project-name-goes-here
```
