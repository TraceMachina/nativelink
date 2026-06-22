{
  writeShellScriptBin,
  trivy,
}:
writeShellScriptBin "trivy-report" ''
  set -euo pipefail

  # Skip trivy scan if SKIP_TRIVY is set
  if [[ "''${SKIP_TRIVY:-false}" != "true" ]]; then
    ${trivy}/bin/trivy \
      image \
      --format sarif \
      ''${TAGGED_IMAGE} \
    > trivy-results.sarif
  else
    echo "Skipping trivy scan (SKIP_TRIVY=true)"
  fi
''
