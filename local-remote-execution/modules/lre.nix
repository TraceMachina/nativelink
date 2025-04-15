{lib, ...}: {
  options = {
    Env = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      description = lib.mdDoc ''
        The environment that makes up the LRE toolchain.

        For instance, to make the the environment from the lre-cc toolchain
        available through the `local-remote-execution.installationScript`:

        ```nix
        inherit (lre-cc.meta) Env;
        ```

        To add a tool to the local execution environment withoug adding it to
        the remote environment:

        ```nix
        Env = [ pkgs.some_local_tool ];
        ```

        If you require toolchains that aren't available in the default LRE
        setups:

        ```
        let
          // A custom remote execution container.
          myCustomToolchain = let
            Env = [
              "PATH=''${pkgs.somecustomTool}/bin"
            ];
          in nix2container.buildImage {
            name = "my-custom-toolchain";
            maxLayers = 100;
            config = {inherit Env;};
            meta = {inherit Env;};
          };
        in
        Env = lre-cc.meta.Env ++ myCustomToolchain.meta.Env;
        ```

        The evaluated contents of `Env` are printed in `lre.bazelrc`, this
        causes nix to put these dependencies into your local nix store, but
        doesn't influence any other tooling like Bazel builds.
      '';
      default = [];
    };
    prefix = lib.mkOption {
      type = lib.types.str;
      description = lib.mdDoc ''
        An optional Bazel config prefix for the flags in `lre.bazelrc`.

        If set, builds need to explicitly enable the LRE config via
        `--config=<prefix>`.

        Defaults to an empty string, enabling LRE by default.
      '';
      default = "";
    };
  };
}
