# Integration tests

These tests run in Kubernetes and are gated behind platform constraints:

```bash
./deployment-examples/kubernetes/00_infra.sh
./deployment-examples/kubernetes/01_operations.sh

bazel test integration_tests --platforms=@rules_nixpkgs_core//platforms:host
```
