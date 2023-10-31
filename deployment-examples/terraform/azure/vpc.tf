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

# --- Begin Endpoints ---

resource "azurerm_virtual_network" "virtual_network" {
  name                = "vnet"
  location            = data.azurerm_resource_group.location
  resource_group_name = data.azurerm_resource_group.name
  address_space       = ["10.0.0.0/16"]
}

resource "azurerm_private_endpoint" "interface_endpoint" {
  name                = "Interface"
  location            = data.azurerm_resource_group.location
  resource_group_name = data.azurerm_resource_group.name

  subnet_id = data.azurerm_subnet.id

  private_service_connection {
    name                           = "private-connection"
    private_connection_resource_id = data.azurerm_linux_virtual_machine.id
    subresource_names              = ["build_turbo_cache_instance"]
    is_manual_connection           = false
  }
}

resource "azurerm_private_endpoint" "blob_endpoint" {
  name                = "blob-endpoint"
  location            = data.azurerm_resource_group.location
  resource_group_name = data.azurerm_resource_group.name

  subnet_id = azurerm_subnet.example.id

  private_service_connection {
    name                           = "blob-connection"
    private_connection_resource_id = data.azurerm_linux_virtual_machine.id
    subresource_names              = ["blob"]
    is_manual_connection           = false
  }
}

# --- End Endpoints ---
