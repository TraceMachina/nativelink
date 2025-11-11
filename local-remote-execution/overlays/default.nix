{nix2container}: final: _prev: {
  inherit (nix2container.packages.${final.system}) nix2container;

  rbe-configs-gen = final.callPackage ./rbe-configs-gen {};

  rbe-autogen = final.callPackage ./rbe-autogen.nix {
    inherit (final.lre) stdenv;
  };

  lre =
    {
      stdenv = final.callPackage ./stdenv.nix {
        inherit (final) llvmPackages;
        targetPackages = final;
      };

      clang = final.callPackage ./clang.nix {
        inherit (final.lre) stdenv;
      };

      lre-cc = final.callPackage ./lre-cc.nix {};
    }
    // (let
      rustConfig = import ./rust-config.nix;

      stableRustFor = p: (rustConfig.mkRust {
        execPkgs = p;
        channel = "stable";
      });

      nightlyRustFor = p: (rustConfig.mkRust {
        execPkgs = p;
        channel = "nightly";
        extensions = ["llvm-tools"];
      });

      stable-rust = stableRustFor final;
      nightly-rust = nightlyRustFor final;
    in {
      inherit stable-rust nightly-rust stableRustFor nightlyRustFor;

      lre-rs = final.callPackage ./lre-rs.nix {
        inherit (rustConfig) nixSystemToRustTargets;
      };
    });
}
