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
  vendorHash = "sha256-QnxFMS9vKwB/oMQdJwUFi9375hbl0IFHvgwX8IEuu3E=";
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
