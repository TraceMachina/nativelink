{pkgs, ...}:
pkgs.buildGoModule {
  pname = "native-cli";
  version = "0.4.0";
  src = ./.;
  vendorHash = "sha256-zB+gaJB+5KEnkPHX2BY8nbO/oOmPk4lfmGzdPBMOSxE=";
  buildInputs = [pkgs.makeWrapper];
  ldflags = ["-s -w"];
  installPhase = ''
    runHook preInstall
    install -D $GOPATH/bin/native-cli $out/bin/native
    runHook postInstall
  '';
  postInstall = let
    pulumiPath = pkgs.lib.makeBinPath [
      (pkgs.pulumi.withPackages (ps: [ps.pulumi-language-go]))
    ];
  in ''
    wrapProgram $out/bin/native --prefix PATH : ${pulumiPath}
  '';
  meta = with pkgs.lib; {
    description = "NativeLink development cluster.";
    homepage = "https://github.com/TraceMachina/nativelink";
    license = licenses.asl20;
    maintainers = [maintainers.aaronmondal];
  };
}
