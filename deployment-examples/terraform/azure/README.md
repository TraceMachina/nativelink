# Turbo Cache's Azure Terraform Deployment
This directory contains a reference/starting point for creating a full Azure Terraform deployment of Turbo Cache's cache and remote execution system.

## Prerequisites - Setup DNS Zone / Base Domain
You are required to first set up an Azure DNS Zone. This is necessary for generating SSL certificates and registering them under a domain.

1. Log in to the Azure portal and go to [Azure DNS Zones](https://portal.azure.com/#view/HubsExtension/BrowseResource/resourceType/Microsoft.Network%2FdnsZones)
2. Click `+ Create`
3. Fill in the necessary details
    1. Resource group (e.g., turbo_cache_infra)
    2. Instance details (e.g., domain name as name, West US
    3. Tags (e.g., turbo_cache:instance_type=avm_builder, environment=dev)
    4. Review and Create -> then Create if satisfied.
4. Once the DNS Zone is created, go into its details to find the `Name servers`
5. In the DNS server where your domain is currently hosted, create a new `NS` record with the name servers you got from Azure.

Propagation may take some time.

## Terraform Setup
1. [Install terraform](https://www.terraform.io/downloads)
2. Open a terminal and run `terraform init` in this directory
3. Run `terraform plan to sanity check that all your variables are defined or populated. You may need to make some changes to common.tf to match your values.`
4. Run `terraform apply -var base_domain=INSERT_DOMAIN_NAME_YOU_SETUP_IN_PREREQUISITES_HERE`

The deployment will take some time. Once completed, your endpoints will be:
```
CAS: grpcs://cas.INSERT_DOMAIN_NAME_YOU_SETUP_IN_PREREQUISITES_HERE
Scheduler: grpcs://scheduler.INSERT_DOMAIN_NAME_YOU_SETUP_IN_PREREQUISITES_HERE
```

To compile this project using Bazel, you can use the following command:
```sh
bazel test //... \
    --remote_cache=grpcs://cas.INSERT_DOMAIN_NAME_YOU_SETUP_IN_PREREQUISITES_HERE \
    --remote_executor=grpcs://scheduler.INSERT_DOMAIN_NAME_YOU_SETUP_IN_PREREQUISITES_HERE \
    --remote_instance_name=main
```

## Server Configuration

## Instances
All instances use the same Azure VM image. This solution is configured to spawn workers for both x86 and ARM architectures, requiring two separate VM images.

### CAS
The CAS serves as a public interface to the Azure Blob Storage. Services will talk to the Blob Storage directly and do not need to interface with the CAS instance.

#### More Optimal Configuration
Similar to the AWS setup, you can reduce costs and enhance reliability by running CAS locally on the machine that invokes remote execution protocols. Update the configuration to point to `localhost`.

### Scheduler
The scheduler remains a single point of failure in this system. Azure Load Balancer could be used here, but it is omitted to save costs and because data encryption is not required within the VNET.

### Workers
Worker instances are optimized for cost, typically utilizing machines with 1 or 2 CPUs and fast local disk storage. Upon initialization, the instance will report its capabilities to the scheduler.

## Security
Security permissions are strictly controlled. Be aware that the VM instances are public-facing by default, and incoming traffic on port 22 (SSH) is allowed for debugging.

Before running `terraform destroy`, you'll need to manually purge the Azure Blob storage, or modify the Terraform configuration to force its deletion.

## Python Script

We need to install a few packages to execute the Python script in `scripts/update_scheduler_ips.py`. To do so, run:
```sh
pip install azure-mgmt-compute azure-mgmt-dns azure-mgmt-resource azure-identity
```

## Future Work / TODOs
* File deletion within Azure Blob Storage is not yet configured and requires careful implementation.
* Auto-scaling is not set up. Monitoring through Azure Monitor and scaling rules will need to be configured.
* Spell out by workstation, the simplest way to configure a Pythonic environment. 

## Useful Tips
You can add `-var terminate_vm_image_builder=false` to the `terraform apply` command for easier testing and modification of these `.tf` files. This prevents VM image builder instances from being terminated, saving time but incurring extra costs.
