{
  config,
  lib,
  pkgs,
  ...
}: let
  pathString = builtins.concatStringsSep ":" config.path;
  bazelrc = pkgs.writeText "nixos.bazelrc" ''
    build --action_env=PATH=${pathString}
    build --host_action_env=PATH=${pathString}
  '';
in {
  options = {
    installationScript = lib.mkOption {
      type = lib.types.str;
      description = "A bash snippet which creates a nixos.bazelrc file in the
        repository.";
    };
    path = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = "List of paths to include in the Bazel environment.";
    };
  };
  config = {
    installationScript = ''
      if ! type -t git >/dev/null; then
        # In pure shells
        echo 1>&2 "WARNING: nixos: git command not found; skipping installation."
      elif ! ${pkgs.git}/bin/git rev-parse --git-dir &> /dev/null; then
        echo 1>&2 "WARNING: nixos: .git not found; skipping installation."
      else
        GIT_WC=`${pkgs.git}/bin/git rev-parse --show-toplevel`

        # These update procedures compare before they write, to avoid
        # filesystem churn. This improves performance with watch tools like
        # lorri and prevents installation loops by lorri.

        if ! readlink "''${GIT_WC}/nixos.bazelrc" >/dev/null \
          || [[ $(readlink "''${GIT_WC}/nixos.bazelrc") != ${bazelrc} ]]; then
          echo 1>&2 "nixos: updating $PWD repository"
          [ -L nixos.bazelrc ] && unlink nixos.bazelrc

          if [ -e "''${GIT_WC}/nixos.bazelrc" ]; then
            echo 1>&2 "nixos: WARNING: Refusing to install because of pre-existing nixos.bazelrc"
            echo 1>&2 "  Remove the nixos.bazelrc file and add nixos.bazelrc to .gitignore."
          else
            ln -fs ${bazelrc} "''${GIT_WC}/nixos.bazelrc"
          fi
        fi
      fi
    '';
  };
}
