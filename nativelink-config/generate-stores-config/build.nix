{craneLib}:
craneLib.buildPackage {
  name = "generate-stores-config";
  src = craneLib.cleanCargoSource ./.;
}
