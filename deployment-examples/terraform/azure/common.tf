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

# --- Begin commonly account variables ---

data "azurerm_client_config" "current" {}

data "azurerm_virtual_network" "main" {
  name                = data.azurerm_client_config.current.virtual_network_name
  resource_group_name = data
}

data "azurerm_subnet" "main" {
  name                 = data.azurerm_client_config.current.subnet_name
  virtual_network_name = data.azurerm_client_config.current.virtual_network_name
  resource_group_name  = data.azurerm_client_config.current.resource_group_name
}

data "azurerm_dns_zone" "main" {
  name                = var.base_domain
  resource_group_name = var.azurerm_client_config.current.resource_group_name
}

data "azurerm_resource_group" "existing" {
  name = "turbo_cache_infra"
  location = "westus"
}

data "azurerm_storage_account" "storage" {
  name = "turbo-cache-storage"
  resource_group_name = data.azurerm_resource_group.existing.name
}


# --- End commonly account variables ---
