# Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

# --- Begin CAS Domain Records ---

resource "azurerm_dns_txt_record" "cas_lb_domain_record_cert_verify" {
  for_each             = { for dvo in azurerm_key_vault_certificate.cas_lb_certificate.dns_validation_options : dvo.domain_name => { name = dvo.record_name, value = dvo.record_value, type = "TXT" } }
  name                = each.value.name
  resource_group_name = var.resource_group_name
  zone_name           = data.azurerm_dns_zone.main.name
  ttl                 = 60
  record {
    value = each.value.value
  }
}

resource "azurerm_dns_a_record" "cas_lb_domain_record" {
  name                = var.cas_domain_prefix
  resource_group_name = var.resource_group_name
  zone_name           = data.azurerm_dns_zone.main.name
  ttl                 = 300
  records             = [azurerm_public_ip.cas_load_balancer.ip_address]
}

# --- End CAS Domain Records ---
# --- Begin Scheduler Domain Records ---

resource "azurerm_dns_txt_record" "scheduler_lb_domain_record_cert_verify" {
  for_each             = { for dvo in azurerm_key_vault_certificate.scheduler_lb_certificate.dns_validation_options : dvo.domain_name => { name = dvo.record_name, value = dvo.record_value, type = "TXT" } }
  name                = each.value.name
  resource_group_name = var.resource_group_name
  zone_name           = data.azurerm_dns_zone.main.name
  ttl                 = 60
  record {
    value = each.value.value
  }
}

resource "azurerm_dns_a_record" "scheduler_lb_domain_record" {
  name                = var.scheduler_domain_prefix
  resource_group_name = var.resource_group_name
  zone_name           = data.azurerm_dns_zone.main.name
  ttl                 = 300
  records             = [azurerm_public_ip.scheduler_load_balancer.ip_address]
}

# --- End Scheduler Domain Records ---
