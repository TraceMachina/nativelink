{nix2container}: final: _prev: {
  inherit (nix2container.packages.${final.stdenv.hostPlatform.system}) nix2container;

  # Note: Only put tools here that should be usable from external flakes.
  nativelink-tools = let
    trivy-report = final.callPackage ./trivy-report.nix {};
  in {
    local-image-test = final.callPackage ./local-image-test.nix {
      skopeo = nix2container.packages.${final.stdenv.hostPlatform.system}.skopeo-nix2container;
    };
    publish-ghcr = final.callPackage ./publish-ghcr.nix {inherit trivy-report;};
    create-local-image = final.callPackage ./create-local-image.nix {};
    create-multi-arch-image = final.callPackage ./create-multi-arch-image.nix {
      regclient = final.callPackage ../regclient.nix {};
      inherit trivy-report;
    };
    regctl-ghcr-login = final.callPackage ./regctl-ghcr-login.nix {
      regclient = final.callPackage ../regclient.nix {};
    };

    lib = {
      createWorker = self:
        final.callPackage (import ./create-worker.nix).createWorker {
          inherit self;
        };
    };
  };
}
