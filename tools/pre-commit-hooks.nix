{ pkgs, ... }:
let

excludes = ["^gencargo/" "^nativelink-proto/genproto"];

in
{
  # Default hooks
  trailing-whitespace-fixer = {
    inherit excludes;
    enable = true;
    name = "trailing-whitespace";
    description = "Remove trailing whitespace";
    entry = "${pkgs.python311Packages.pre-commit-hooks}/bin/trailing-whitespace-fixer";
    types = [ "text" ];
  };
  end-of-file-fixer = {
    inherit excludes;
    enable = true;
    name = "end-of-file-fixer";
    description = "Remove trailing whitespace";
    entry = "${pkgs.python311Packages.pre-commit-hooks}/bin/end-of-file-fixer";
    types = [ "text" ];
  };
  fix-byte-order-marker = {
    inherit excludes;
    enable = true;
    name = "fix-byte-order-marker";
    entry = "${pkgs.python311Packages.pre-commit-hooks}/bin/fix-byte-order-marker";
  };
  mixed-line-ending = {
    inherit excludes;
    enable = true;
    name = "mixed-line-ending";
    entry = "${pkgs.python311Packages.pre-commit-hooks}/bin/mixed-line-ending";
    types = [ "text" ];
  };
  check-case-conflict = {
    inherit excludes;
    enable = true;
    name = "check-case-conflict";
    entry = "${pkgs.python311Packages.pre-commit-hooks}/bin/check-case-conflict";
    types = [ "text" ];
  };
  detect-private-key = {
    excludes = excludes ++ [
      # This is an integration testfile that is not intended for production.
      "deployment-examples/docker-compose/example-do-not-use-in-prod-key.pem"
    ];
    enable = true;
    name = "detect-private-key";
    entry = "${pkgs.python311Packages.pre-commit-hooks}/bin/detect-private-key";
    types = [ "text" ];
  };

  # Starlark
  bazel-buildifier-format = {
    enable = true;
    name = "buildifier format";
    description = "Format Starlark";
    entry = "${pkgs.bazel-buildtools}/bin/buildifier -lint=fix";
    types = [ "bazel" ];
  };
  bazel-buildifier-lint = {
    enable = true;
    name = "buildifier lint";
    description = "Lint Starlark";
    entry = "${pkgs.bazel-buildtools}/bin/buildifier -lint=warn";
    types = [ "bazel" ];
  };
}
