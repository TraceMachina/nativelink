# Copyright 2022 The NativeLink Authors. All rights reserved.
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

resource "aws_route53_record" "cas_lb_domain_record_cert_verify" {
  for_each = {
    for dvo in aws_acm_certificate.cas_lb_certificate.domain_validation_options : dvo.domain_name => {
      name   = dvo.resource_record_name
      record = dvo.resource_record_value
      type   = dvo.resource_record_type
    }
  }
  name    = each.value.name
  records = [each.value.record]
  type    = each.value.type

  zone_id = data.aws_route53_zone.main.zone_id
  ttl     = 60
}

resource "aws_route53_record" "cas_lb_domain_record" {
  zone_id = data.aws_route53_zone.main.zone_id
  name    = "${var.cas_domain_prefix}.${var.base_domain}"
  type    = "A"

  alias {
    name                   = aws_lb.cas_load_balancer.dns_name
    zone_id                = aws_lb.cas_load_balancer.zone_id
    evaluate_target_health = true
  }
}

# --- End CAS Domain Records ---
# --- Begin Scheduler Domain Records ---

resource "aws_route53_record" "scheduler_lb_domain_record_cert_verify" {
  for_each = {
    for dvo in aws_acm_certificate.scheduler_lb_certificate.domain_validation_options : dvo.domain_name => {
      name   = dvo.resource_record_name
      record = dvo.resource_record_value
      type   = dvo.resource_record_type
    }
  }
  name    = each.value.name
  records = [each.value.record]
  type    = each.value.type

  zone_id = data.aws_route53_zone.main.zone_id
  ttl     = 60
}

resource "aws_route53_record" "scheduler_lb_domain_record" {
  zone_id = data.aws_route53_zone.main.zone_id
  name    = "${var.scheduler_domain_prefix}.${var.base_domain}"
  type    = "A"

  alias {
    name                   = aws_lb.scheduler_load_balancer.dns_name
    zone_id                = aws_lb.scheduler_load_balancer.zone_id
    evaluate_target_health = true
  }
}

# --- End Scheduler Domain Records ---
