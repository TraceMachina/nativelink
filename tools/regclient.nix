# FIXME: Based off of https://github.com/NixOS/nixpkgs/blob/nixos-26.05/pkgs/by-name/re/regclient/package.nix
# plus https://github.com/regclient/regclient/pull/1106 for better "not found"
{
  lib,
  buildGoModule,
  fetchFromGitHub,
  installShellFiles,
  lndir,
  testers,
  regclient,
}: let
  bins = [
    "regbot"
    "regctl"
    "regsync"
  ];
in
  buildGoModule rec {
    pname = "regclient";
    version = "0.11.5";
    tag = "v${version}";

    src = fetchFromGitHub {
      owner = "regclient";
      repo = "regclient";
      rev = "8a3d428f8503450580f60e71f08ac4cecfdcc01d";
      sha256 = "sha256-Ho5XKxK+xEhnUryZY0KGcu19fH26nPn6DqsDsoW30p4=";
    };
    vendorHash = "sha256-5SI1c0yszMTiC20bBXKIrtHVuDWA9BaSC5RY0F8B1Us=";

    outputs = ["out"] ++ bins;

    ldflags = [
      "-s"
      "-w"
      "-X github.com/regclient/regclient/internal/version.vcsTag=${tag}"
    ];

    nativeBuildInputs = [
      installShellFiles
      lndir
    ];

    postInstall =
      lib.concatMapStringsSep "\n" (bin: ''
        export bin=''$${bin}
        export outputBin=bin

        mkdir -p $bin/bin
        mv $out/bin/${bin} $bin/bin

        installShellCompletion --cmd ${bin} \
          --bash <($bin/bin/${bin} completion bash) \
          --fish <($bin/bin/${bin} completion fish) \
          --zsh <($bin/bin/${bin} completion zsh)

        lndir -silent $bin $out

        unset bin outputBin
      '')
      bins;

    doCheck = false;

    checkFlags = [
      # touches network
      "-skip=^ExampleNew$"
    ];

    passthru.tests = lib.mergeAttrsList (
      map (bin: {
        "${bin}Version" = testers.testVersion {
          package = regclient;
          command = "${bin} version";
          version = tag;
        };
      })
      bins
    );

    __darwinAllowLocalNetworking = true;

    meta = {
      description = "Docker and OCI Registry Client in Go and tooling using those libraries";
      homepage = "https://github.com/regclient/regclient";
      license = lib.licenses.asl20;
      maintainers = with lib.maintainers; [maxbrunet];
    };
  }
