{
  stdenvNoCC,
  fetchPnpmDeps,
  pnpmConfigHook,
  pnpm_11,
  nodejs_22,
}:
stdenvNoCC.mkDerivation (finalAttrs: {
  pname = "prettier-node-modules";
  version = "0.0.0";

  src = ./.;

  # The pnpm store is fetched for every platform at once, so this one hash
  # holds on Linux and macOS alike. Update it every time pnpm-lock.yaml
  # changes: set it to "" and copy the hash Nix reports.
  pnpmDeps = fetchPnpmDeps {
    inherit (finalAttrs) pname version src;
    pnpm = pnpm_11;
    fetcherVersion = 4;
    hash = "sha256-eqXuHiwjRXkuh3uijcBGJSpOOVQ/FJoLWEvHflqxJoA=";
  };

  nativeBuildInputs = [
    nodejs_22
    pnpm_11
    pnpmConfigHook
  ];

  # pnpmConfigHook installs node_modules from the prefetched store, offline.
  dontBuild = true;

  installPhase = ''
    runHook preInstall
    mkdir -p $out
    cp package.json pnpm-lock.yaml .prettierrc.json $out/
    cp -r node_modules $out/
    runHook postInstall
  '';

  dontFixup = true;
})
