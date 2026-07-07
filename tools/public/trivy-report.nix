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
      $1 \
    > trivy-results.sarif
  else
    echo "Skipping trivy scan (SKIP_TRIVY=true)"
  fi
''
