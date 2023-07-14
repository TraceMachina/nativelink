{
  description = "turbo-cache";

  nixConfig = {
    bash-prompt-prefix = "(turbo-cache) ";
    bash-prompt = ''\[\033]0;\u@\h:\w\007\]\[\033[01;32m\]\u@\h\[\033[01;34m\] \w \$\[\033[00m\]'';
    bash-prompt-suffix = " ";
  };

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
    pre-commit-hooks-nix.url = "github:cachix/pre-commit-hooks.nix";

    # Use upstream rules_ll. This is highly unstable and we're only doing this
    # because we know it's being developed "in sync" with turbo-cache.
    rules_ll.url = "github:eomii/rules_ll";
  };

  outputs =
    { self
    , nixpkgs
    , flake-utils
    , pre-commit-hooks-nix
    , rules_ll
    , ...
    } @ inputs:
    flake-utils.lib.eachSystem [
      "x86_64-linux"
    ]
      (system:
      let
        pkgs = import nixpkgs { inherit system; };
        hooks = import ./pre-commit-hooks.nix { inherit pkgs; };
        llShell = rules_ll.lib.${system}.llShell;

        openssl_static = (pkgs.openssl.override { static = true; });

        # Not using the pinned bazelWrapper explicitly here since rules_ll needs
        # to improve logic around that first.
        cmd = (string: "\\E[32m" + string + "\\033[0m");
        fat = (string: "\\E[1m" + string + "\\033[0m");

        ll = pkgs.writeShellScriptBin "ll" ''
          if [[ "$1" == "up" ]]; then
          PATH=''${PATH+$PATH:}${pkgs.pulumi}/bin \
            bazel run @rules_ll//devtools:cluster -- up
          elif [[ "$1" == "down" ]]; then
          PATH=''${PATH+$PATH:}${pkgs.pulumi}/bin \
            bazel run @rules_ll//devtools:cluster -- down
          else

          printf '
          The ${fat "ll"} development tool for rules_ll.

          ll ${cmd "up"}:\tStart the development cluster. If a cluster is already running, refresh it.

          ll ${cmd "down"}:\tStop and delete the development cluster.
          '

          fi
        '';
      in
      {
        checks = {
          pre-commit-check = pre-commit-hooks-nix.lib.${system}.run {
            src = ./.;
            inherit hooks;
          };
        };

        devShells = {
          default = llShell {
            unfree = false; # We don't need to support NVPTX.
            packages = [
              # Don't import Bazel here - it's already part of the llShell and
              # shouldn't be overridden carelessly.

              # Adds the `ll up` and `ll down` commands to start and delete the
              # development cluster.
              ll

              # Default tooling.
              pkgs.git

              # Cloud tooling
              pkgs.cilium-cli
              pkgs.kubectl
              pkgs.pulumi
              pkgs.skopeo
              pkgs.tektoncd-cli
            ];
            env = {
              # Override the version in .bazelrc which might not always be in
              # sync with the bazel wrapper from rules_ll.
              USE_BAZEL_VERSION = "6.2.0";

              # Required for the "openssl-sys" crate. Every time you update the
              # flake the corresponding annotations in the WORKSPACE need to be
              # updated. Not ideal but it'll have to do for now.
              OPENSSL_INCLUDE_DIR = "${openssl_static.dev}/include";
              OPENSSL_LIB_DIR = "${openssl_static.out}/lib";
            };
            inherit hooks;
          };
        };
      });
}
