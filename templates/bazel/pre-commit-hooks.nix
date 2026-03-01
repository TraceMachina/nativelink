{pkgs, ...}: let
  excludes = [];
in {
  # General
  check-case-conflicts = {
    enable = true;
    inherit excludes;
    types = ["text"];
  };
  detect-private-keys = {
    enable = true;
    inherit excludes;
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
    inherit excludes;
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

  # C++
  clang-format.enable = true;

  # Nix
  alejandra.enable = true;
  deadnix.enable = true;
  statix.enable = true;

  # Starlark
  bazel-buildifier-format = {
    enable = true;
    entry = "${pkgs.bazel-buildtools}/bin/buildifier -lint=fix";
    name = "buildifier format";
    types = ["bazel"];
  };
  bazel-buildifier-lint = {
    enable = true;
    entry = "${pkgs.bazel-buildtools}/bin/buildifier -lint=warn";
    excludes = ["local-remote-execution/generated-cc/cc/cc_toolchain_config.bzl"];
    name = "buildifier lint";
    types = ["bazel"];
  };
}
