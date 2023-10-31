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

resource "azurerm_linux_virtual_machine" "build_turbo_cache_instance" {
  for_each = {
    arm = {
      "vm_size": var.build_arm_vm_size,
      "image_id": var.build_base_image_arm,
    },
    x86 = {
      "vm_size": var.build_x86_vm_size,
      "image_id": var.build_base_image_x86,
    }
  }

  name                  = "turbo_cache${each.key}"
  resource_group_name = data.azurerm_resource_group.existing.name
  location            = data.azurerm_resource_group.existing.location
  network_interface_ids = [azurerm_network_interface.example.id]
  size                  = each.value["vm_size"]
  admin_username        = "ubuntu"
  disable_password_authentication = true

  os_disk {
    name              = "osdisk"
    caching           = "ReadWrite"
    storage_account_type = "Standard_LRS"
  }

  tags = {
    "turbo_cache_instance_type" = "avm_builder"
  }

  connection {
    host        = data.azurerm_public_ip.example.ip_address
    type        = "ssh"
    user        = "ubuntu"
    private_key = file("~/.ssh/id_rsa")
  }

}

resource "azurerm_image" "base_image" {
  for_each = {
    arm = "arm",
    x86 = "x86"
  }

  name                = "turbo_cache_${each.key}_base"
  resource_group_name = data.azurerm_resource_group.name
  location            = data.azurerm_resource_group.location

  os_disk {
    os_type  = "Linux"
    os_state = "Generalized"
    blob_uri = azurerm_linux_virtual_machine.build_turbo_cache_instance[each.key].os_disk.0.managed_disk_id
    size_gb  = 30
  }
}

##########
#
#
#
# source_image_reference {
#       publisher = each.value["image"]["publisher"]
#       offer     = each.value["image"]["offer"]
#       sku       = each.value["image"]["sku"]
#       version   = each.value["image"]["version"]
#     }
##########
