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

resource "tls_private_key" "ssh_key" {
  algorithm = "ED25519"
}

resource "aws_key_pair" "nativelink_key" {
  key_name   = "nativelink-key"
  public_key = data.tls_public_key.nativelink_pem.public_key_openssh
}

data "tls_public_key" "nativelink_pem" {
  private_key_openssh = tls_private_key.ssh_key.private_key_openssh
  # This is left here for convenience. Comment out the line above and uncomment and modify this
  # line to use a custom SSH key.
  # private_key_openssh = file(pathexpand("~/.ssh/id_rsa"))
}
