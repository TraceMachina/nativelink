final: _prev: {
  lre = {
    stdenv = final.callPackage ./stdenv.nix {
      llvmPackages = final.llvmPackages_19;
      targetPackages = final;
    };

    clang = final.callPackage ./clang.nix {
      inherit (final.lre) stdenv;
    };
  };
}
