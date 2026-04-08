{
  pkgs,
  nightly-rust,
  generate-bazel-rc,
  generate-stores-config,
  renovate-patched,
  ...
}: let
  excludes = ["nativelink-proto/genproto"];
in {
  # Default hooks
  check-case-conflicts = {
    enable = true;
    inherit excludes;
    types = ["text"];
  };
  detect-private-keys = {
    enable = true;
    excludes =
      excludes
      ++ [
        # Integration testfiles not intended for production.
        "deployment-examples/docker-compose/example-do-not-use-in-prod-key.pem"
        "kubernetes/resources/insecure-certs/example-do-not-use-in-prod-key.pem"
      ];
    types = ["text"];
  };
  end-of-file-fixer = {
    enable = true;
    inherit excludes;
    types = ["text"];
  };
  fix-byte-order-marker = {
    enable = true;
    inherit excludes;
  };
  forbid-binary-files = {
    enable = true;
    entry = let
      script = pkgs.writeShellScriptBin "forbid-binary-files" ''
        set -eu

        if [ $# -gt 0 ]; then
          for filename in "''${@}"; do
            printf "[\033[31mERROR\033[0m] Found binary file: ''${filename}"
          done
          exit 1
        fi
      '';
    in "${script}/bin/forbid-binary-files";
    excludes = [
      # Testdata for fastcdc.
      "nativelink-util/tests/data/SekienAkashita.jpg"
    ];
    name = "forbid-binary-files";
    types = ["binary"];
  };
  mixed-line-endings = {
    enable = true;
    inherit excludes;
    types = ["text"];
  };
  trim-trailing-whitespace = {
    enable = true;
    inherit excludes;
    types = ["text"];
  };

  # Dockerfile
  hadolint.enable = true;

  # Documentation
  vale = {
    enable = true;
    inherit excludes;
    settings.configPath = ".vale.ini";
  };

  # General
  typos = {
    enable = true;
    settings.configPath = "typos.toml";
  };

  # Nix
  alejandra.enable = true;
  deadnix.enable = true;
  statix.enable = true;

  # Rust
  rustfmt = {
    enable = true;
    packageOverrides.cargo = nightly-rust.cargo;
    packageOverrides.rustfmt = nightly-rust.rustfmt;
    pass_filenames = true;
    inherit excludes;
  };

  # Taplo fmt
  taplo = {
    enable = true;
    types = ["toml"];
  };

  # Taplo validate
  taplo-validate = {
    enable = true;
    entry = "${pkgs.taplo}/bin/taplo validate";
    name = "taplo validate";
    types = ["toml"];
  };

  # Shell
  shellcheck = {
    enable = true;
    excludes = [".envrc"] ++ excludes;
  };
  shfmt = {
    args = ["--indent" "4" "--space-redirects"];
    enable = true;
    inherit excludes;
  };

  # Starlark
  bazel-buildifier-format = {
    description = "Format Starlark";
    enable = true;
    entry = "${pkgs.bazel-buildtools}/bin/buildifier -lint=fix";
    name = "buildifier format";
    types = ["bazel"];
  };
  bazel-buildifier-lint = {
    description = "Lint Starlark";
    enable = true;
    entry = "${pkgs.bazel-buildtools}/bin/buildifier -lint=warn";
    excludes = ["local-remote-execution/generated-cc/cc/cc_toolchain_config.bzl"];
    name = "buildifier lint";
    types = ["bazel"];
  };

  # bazelrc
  generate-bazel-rc = {
    description = "Generate bazelrc";
    enable = true;
    entry = "${generate-bazel-rc}/bin/generate-bazel-rc Cargo.toml .bazelrc";
    name = "generate-bazel-rc";
    files = "Cargo.toml|.bazelrc";
    pass_filenames = false;
  };

  pretty-format-json = {
    enable = true;
    args = ["--autofix" "--top-keys" "name,type"];
  };

  # json5
  formatjson5 = {
    excludes =
      excludes
      ++ ["nativelink-config/examples/stores-config.json5"];
    description = "Format json5 files";
    enable = true;
    entry = "${pkgs.formatjson5}/bin/formatjson5";
    args = ["-r" "--indent" "2"];
    types = ["json5"];
  };

  # Renovate config validator
  renovate = {
    description = "Validate renovate config";
    enable = true;
    entry = "${renovate-patched}/bin/renovate-config-validator";
    args = ["--strict"];
    files = "renovate.json5";
  };

  # Detect unused cargo deps
  machete = {
    description = "Detect unused cargo deps";
    enable = true;
    entry = "${pkgs.cargo-machete}/bin/cargo-machete";
    args = ["--with-metadata" "."];
    pass_filenames = false;
  };

  # Generate demo config to test stores.rs comments
  generate-stores-config = {
    description = "Generate stores config";
    enable = true;
    entry = "${generate-stores-config}/bin/generate-stores-config nativelink-config/src/stores.rs nativelink-config/examples/stores-config.json5";
    name = "generate-stores-config";
    files = "nativelink-config/src/stores.rs|nativelink-config/examples/stores-config.json5";
    pass_filenames = false;
  };
}
