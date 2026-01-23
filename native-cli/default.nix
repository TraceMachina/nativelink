{
  buildGoModule,
  makeWrapper,
  lib,
  pulumi,
  pulumiPackages,
}:
buildGoModule {
  pname = "native-cli";
  version = "0.6.0";
  src = ./.;
  vendorHash = "sha256-NePX6xJFHu8e8U4iovgvkDc7BoI9ZspAASr/F7l/ZRA=";
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
      pulumiPackages.pulumi-go
    ];
  in ''
    wrapProgram $out/bin/native --prefix PATH : ${pulumiPath}
  '';
  meta = with lib; {
    description = "NativeLink development cluster.";
    homepage = "https://github.com/TraceMachina/nativelink";
    license = licenses.asl20;
    maintainers = [maintainers.aaronmondal maintainers.SchahinRohani];
  };
}
