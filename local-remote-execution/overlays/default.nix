{nix2container}: final: _prev: {
  inherit (nix2container.packages.${final.system}) nix2container;

  rbe-configs-gen = final.callPackage ./rbe-configs-gen {};

  rbe-autogen = final.callPackage ./rbe-autogen.nix {
    inherit (final.lre) stdenv;
  };

  lre = {
    stdenv = final.callPackage ./stdenv.nix {
      llvmPackages = final.llvmPackages_19;
      targetPackages = final;
    };

    clang = final.callPackage ./clang.nix {
      inherit (final.lre) stdenv;
    };

    lre-cc = final.callPackage ./lre-cc.nix {};
  };
}
