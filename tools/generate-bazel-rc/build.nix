{craneLib}:
craneLib.buildPackage {
  name = "generate-bazel-rc";
  src = craneLib.cleanCargoSource ./.;
}
