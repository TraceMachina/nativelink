# Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

"""Update Azure DNS Zone to add IP addresses of instances in VM Scale Set"""

import os
from azure.identity import DefaultAzureCredential
from azure.mgmt.compute import ComputeManagementClient
from azure.mgmt.dns import DnsManagementClient
from azure.mgmt.resource import ResourceManagementClient

RESOURCE_GROUP_NAME = os.environ["RESOURCE_GROUP_NAME"]
SCALE_SET_NAME = os.environ["SCALE_SET_NAME"]
DNS_ZONE_NAME = os.environ["DNS_ZONE_NAME"]
SUBDOMAIN_NAME = os.environ["SUBDOMAIN_NAME"]

# Use Azure AD token credentials to authenticate
credentials = DefaultAzureCredential()

resource_client = ResourceManagementClient(credentials, os.environ["AZURE_SUBSCRIPTION_ID"])
compute_client = ComputeManagementClient(credentials, os.environ["AZURE_SUBSCRIPTION_ID"])
dns_client = DnsManagementClient(credentials, os.environ["AZURE_SUBSCRIPTION_ID"])

def update_dns_record(target_ips):
    """ Change the DNS record """
    records = []
    for target_ip in target_ips:
        records.append({"ipv4_address": target_ip})
    
    dns_client.record_sets.create_or_update(
        RESOURCE_GROUP_NAME,
        DNS_ZONE_NAME,
        SUBDOMAIN_NAME,
        "A",
        {
            "ttl": 300,
            "arecords": records
        }
    )


def main():
    # Get the VM instances in the VM Scale Set
    instance_paged = compute_client.virtual_machine_scale_set_vms.list(
        RESOURCE_GROUP_NAME,
        SCALE_SET_NAME
    )
    
    instance_ids = []
    for instance in instance_paged:
        instance_ids.append(instance.instance_id)
    
    if len(instance_ids) <= 0:
        print("No instances active")
        return
    
    private_ips = []
    # Get the network interfaces for the VM instances
    for instance_id in instance_ids:
        network_interfaces = compute_client.virtual_machine_scale_set_vms.list_network_interfaces(
            RESOURCE_GROUP_NAME,
            SCALE_SET_NAME,
            instance_id
        )
        
        # Extract the private IP addresses
        for nic in network_interfaces:
            for ip_config in nic.ip_configurations:
                if ip_config.private_ip_address:
                    private_ips.append(ip_config.private_ip_address)
    
    # Update DNS Record
    update_dns_record(private_ips)
    return {
        "code": 200,
        "message": f"Updated record to {private_ips}"
    }

if __name__ == "__main__":
    main()
