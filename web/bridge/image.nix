{
  pkgs,
  buildImage,
  pullImage,
  ...
}: let
  imageParams =
    if pkgs.stdenv.hostPlatform.system == "x86_64-linux"
    then {
      imageName = "oven/bun";
      imageDigest = "sha256:2ebe63bae78e24788a7c0be646475ebfa843167370bfae1e436383c15dd70cc7";
      sha256 = "sha256-ZSDLLWWVDmPhTEOu1DkCnhJoODpOTbWOumk38419MRE=";
      arch = "amd64";
      description = "A simple Bun environment image for x86_64-linux.";
      title = "Bun Environment x86_64-linux";
    }
    else if pkgs.stdenv.hostPlatform.system == "aarch64-linux" || pkgs.stdenv.hostPlatform.system == "aarch64-darwin"
    then {
      imageName = "oven/bun";
      imageDigest = "sha256:f83947fb1646bee6f71d7aaf7914cd7ae7eedcb322b10b52aadfc1a3d56da84e";
      sha256 = "sha256-ZSDLLWWVDmPhTEOu1DkCnhJoODpOTbWOumk38419MRE=";
      arch = "arm64";
      description = "A simple Bun environment image for aarch64-linux.";
      title = "Bun Environment aarch64-linux";
    }
    else throw "Unsupported architecture: ${pkgs.stdenv.hostPlatform.system}";

  webBridge = pkgs.stdenv.mkDerivation {
    name = "web-bridge-files";
    src = ./.;
    buildInputs = [pkgs.bun];
    installPhase = ''
      mkdir -p $out
      cp -r * $out
      cd $out
      bun install
    '';
  };
in
  buildImage {
    name = "nativelink-web-bridge";

    # Base image configuration
    fromImage = pullImage {
      inherit (imageParams) imageName imageDigest sha256 arch;
      tlsVerify = true;
      os = "linux";
    };

    # Container configuration
    config = {
      WorkingDir = "/web/bridge";
      Entrypoint = ["bun" "run" "${webBridge}/index.ts"];
      ExposedPorts = {
        "8080/tcp" = {};
      };
      Labels = {
        "org.opencontainers.image.description" = imageParams.description;
        "org.opencontainers.image.title" = imageParams.title;
      };
    };
  }
