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

# --- Begin Azure Function to update internal scheduler ips ---

# This function will be configured to listen to the scheduler virtual machine scale set
# so, when new instances are created/destroyed it will trigger this function.
# The function will query all instances that are part of the scale set
# and add the private IPs of each instance to an Azure DNS Zone record that will
# be used by workers to find the scheduler(s).
resource "azurerm_function_app" "update_scheduler_ips_function" {
  name                      = "turbo_cache_update_scheduler_ips"
  resource_group_name       = azurerm_resource_group.configuration.name
  location                  = azurerm_resource_group.configuration.location
  app_service_plan_id       = azurerm_app_service_plan.configuration.id
  storage_account_name      = azurerm_storage_account.example.name
  storage_account_access_key = azurerm_storage_account.example.primary_access_key
  os_type                   = "linux"
  version                   = "~3"
  
  app_settings = {
    HOSTED_ZONE_ID = azurerm_dns_zone.example.id,
    SCHEDULER_DOMAIN = "${var.scheduler_domain_prefix}.internal.${var.base_domain}",
    VM_SCALE_SET_NAME = azurerm_virtual_machine_scale_set.example.name
  }
}

data "archive_file" "update_scheduler_ips" {
  type        = "zip"
  source_file = "scripts/functions/update_scheduler_ips.py"
  output_path = ".update_scheduler_ips.zip"
}

resource "azurerm_eventgrid_topic" "update_scheduler_ips_eventgrid_topic" {
  name                 = "turbo_cache_scheduler_ips_eventgrid_topic"
  location             = azurerm_resource_group.example.location
  resource_group_name  = azurerm_resource_group.example.name
}

resource "azurerm_eventgrid_event_subscription" "update_scheduler_ips_event_subscription" {
  name  = "turbo_cache_scheduler_ips_event_subscription"
  scope = azurerm_eventgrid_topic.update_scheduler_ips_eventgrid_topic.id
  event_delivery_schema = "EventGridSchema"

  azure_function_endpoint {
    function_id = azurerm_function_app.update_scheduler_ips_function.id
  }
}

# --- End Azure Function to update internal scheduler ips ---
