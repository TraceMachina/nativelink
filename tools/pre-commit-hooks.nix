{
  pkgs,
  nightly-rust,
  generate-bazel-rc,
  ...
}: let
  excludes = ["nativelink-proto/genproto" "native-cli/vendor"];
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
    args = ["--force-exclude"];
    enable = true;
    inherit excludes;
    settings.configPath = "typos.toml";
  };

  # Go
  gci = {
    description = "Fix go imports.";
    enable = true;
    entry = "${pkgs.gci}/bin/gci write";
    inherit excludes;
    name = "gci";
    types = ["go"];
  };
  gofumpt = {
    description = "Format Go.";
    enable = true;
    entry = "${pkgs.gofumpt}/bin/gofumpt -w -l";
    inherit excludes;
    name = "gofumpt";
    types = ["go"];
  };
  # TODO(aaronmondal): This linter works in the nix developmen environment, but
  #                    not with `nix flake check`. It's unclear how to fix this.
  golangci-lint-in-shell = {
    enable = true;
    entry = let
      script = pkgs.writeShellScript "precommit-golangci-lint" ''
        # TODO(aaronmondal): This linter works in the nix development
        #                    environment, but not with `nix flake check`. It's
        #                    unclear how to fix this.
        if [ ''${IN_NIX_SHELL} = "impure" ]; then
          export PATH=${pkgs.go}/bin:$PATH
          cd native-cli
          CC=customClang ${pkgs.golangci-lint}/bin/golangci-lint run --modules-download-mode=readonly
        fi
      '';
    in
      builtins.toString script;
    inherit excludes;
    pass_filenames = false;
    require_serial = true;
    types = ["go"];
  };
  golines = {
    description = "Shorten Go lines.";
    enable = true;
    entry = "${pkgs.golines}/bin/golines --max-len=80 -w";
    inherit excludes;
    name = "golines";
    types = ["go"];
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
  };
  taplo = {
    enable = true;
    excludes = ["nativelink-proto"];
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

  # json5
  formatjson5 = {
    description = "Format json5 files";
    enable = true;
    entry = "${pkgs.formatjson5}/bin/formatjson5";
    args = ["-r" "--indent" "2"];
    types = ["json5"];
  };
}
