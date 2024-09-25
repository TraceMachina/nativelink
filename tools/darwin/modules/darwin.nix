{
  config,
  lib,
  pkgs,
  ...
}: let
  # Conditionally set the bazelrc only if the target system is Darwin (macOS)
  bazelrc =
    if pkgs.stdenv.isDarwin
    then
      pkgs.writeText "darwin.bazelrc" ''
        # These flags are dynamically generated by the Darwin flake module.
        #
        # Add `try-import %%workspace%%/darwin.bazelrc` to your .bazelrc to
        # include these flags when running Bazel in a nix environment.
        # These are the libs and frameworks used by darwin.

        build --@rules_rust//:extra_rustc_flags=-L${pkgs.libiconv}/lib,-Lframework=${pkgs.darwin.Security}/Library/Frameworks,-Lframework=${pkgs.darwin.CF}/Library/Frameworks
        build --@rules_rust//:extra_exec_rustc_flags=-L${pkgs.libiconv}/lib,-Lframework=${pkgs.darwin.Security}/Library/Frameworks,-Lframework=${pkgs.darwin.CF}/Library/Frameworks
      ''
    else "";
in {
  options = {
    installationScript = lib.mkOption {
      type = lib.types.str;
      description = "A bash snippet which creates a darwin.bazelrc file in the
        repository.";
    };
  };
  config = {
    installationScript = ''
      if ! type -t git >/dev/null; then
        # In pure shells
        echo 1>&2 "WARNING: Darwin: git command not found; skipping installation."
      elif ! ${pkgs.git}/bin/git rev-parse --git-dir &> /dev/null; then
        echo 1>&2 "WARNING: Darwin: .git not found; skipping installation."
      else
        GIT_WC=`${pkgs.git}/bin/git rev-parse --show-toplevel`

        # These update procedures compare before they write, to avoid
        # filesystem churn. This improves performance with watch tools like
        # lorri and prevents installation loops by lorri.

        if ! readlink "''${GIT_WC}/darwin.bazelrc" >/dev/null \
          || [[ $(readlink "''${GIT_WC}/darwin.bazelrc") != ${bazelrc} ]]; then
          echo 1>&2 "Darwin: updating $PWD repository"
          [ -L darwin.bazelrc ] && unlink darwin.bazelrc

          if [ -e "''${GIT_WC}/darwin.bazelrc" ]; then
            echo 1>&2 "Darwin: WARNING: Refusing to install because of pre-existing darwin.bazelrc"
            echo 1>&2 "  Remove the darwin.bazelrc file and add darwin.bazelrc to .gitignore."
          else
            ln -fs ${bazelrc} "''${GIT_WC}/darwin.bazelrc"
          fi
        fi
      fi
    '';
  };
}
