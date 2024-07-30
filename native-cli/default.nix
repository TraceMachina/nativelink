{
  buildGoModule,
  makeWrapper,
  lib,
  pulumi,
  pulumiPackages,
}:
buildGoModule {
  pname = "native-cli";
  version = "0.4.0";
  src = ./.;
  vendorHash = "sha256-p5lMhIHP0Al/0adP7/8w2P3a73xK1fIj9k5OApM/LjE=";
  buildInputs = [makeWrapper];
  ldflags = ["-s -w"];
  installPhase = ''
    runHook preInstall
    install -D $GOPATH/bin/native-cli $out/bin/native
    runHook postInstall
  '';
  postInstall = let
    pulumiPath = lib.makeBinPath [
      pulumi
      pulumiPackages.pulumi-language-go
    ];
  in ''
    wrapProgram $out/bin/native --prefix PATH : ${pulumiPath}
  '';
  meta = with lib; {
    description = "NativeLink development cluster.";
    homepage = "https://github.com/TraceMachina/nativelink";
    license = licenses.asl20;
    maintainers = [maintainers.aaronmondal];
  };
}
