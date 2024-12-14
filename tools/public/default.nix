{nix2container}: final: _prev: {
  inherit (nix2container.packages.${final.system}) nix2container;

  # Note: Only put tools here that should be usable from external flakes.
  nativelink-tools = {
    local-image-test = final.callPackage ./local-image-test.nix {};
    publish-ghcr = final.callPackage ./publish-ghcr.nix {};
    native-cli = final.callPackage ../../native-cli/default.nix {};

    lib = {
      createWorker = self:
        final.callPackage (import ./create-worker.nix).createWorker {
          inherit self;
        };
    };
  };
}
