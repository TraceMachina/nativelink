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

resource "azurerm_key_vault_certificate_issuer" "key_issuer" {
  name                = "key-issuer"
  key_vault_id        = azurerm_key_vault.example.id
  provider_name       = "DigiCert"
  account_id          = "your_account_id_here"
  password            = "your_password_here"

  admin {
    first_name      = "example-admin-first-name"
    last_name       = "example-admin-last-name"
    email_address   = "admin@example.com"
    phone           = "example-phone"
  }
}


# CAS Certificate
resource "azurerm_key_vault_certificate" "cas_lb_certificate" {
  name         = "cas-lb-certificate"
  key_vault_id = azurerm_key_vault.example.id

  certificate_policy {
    issuer_parameters {
      name = azurerm_key_vault_certificate_issuer.key_issuer.name
    }

    key_properties {
      exportable = true
      key_type   = "RSA"
      key_size   = 2048
      reuse_key  = false
    }

    lifetime_action {
      action {
        action_type = "AutoRenew"
      }

      trigger {
        days_before_expiry = 30
      }
    }

    secret_properties {
      content_type = "application/x-pkcs12"
    }

    x509_certificate_properties {
      subject = "CN=${var.scheduler_domain_prefix}.${var.base_domain}"
      validity_in_months = 12
      key_usage = azurerm_linux_virtual_machine.build_turbo_cache_instance.os_profile_linux_config.ssh_keys
    }
  }
}

# Scheduler Certificate
resource "azurerm_key_vault_certificate" "scheduler_lb_certificate" {
  name         = "scheduler-lb-certificate"
  key_vault_id = azurerm_key_vault.example.id

  certificate_policy {
    issuer_parameters {
      name = azurerm_key_vault_certificate_issuer.key_issuer.name
    }

    key_properties {
      exportable = true
      key_type   = "RSA"
      key_size   = 2048
      reuse_key  = false
    }

    lifetime_action {
      action {
        action_type = "AutoRenew"
      }

      trigger {
        days_before_expiry = 30
      }
    }

    secret_properties {
      content_type = "application/x-pkcs12"
    }

    x509_certificate_properties {
      subject = "CN=${var.scheduler_domain_prefix}.${var.base_domain}"
      validity_in_months = 12
      key_usage = azurerm_linux_virtual_machine.build_turbo_cache_instance.os_profile_linux_config.ssh_keys
    }
  }
}
