{lib, ...}: {
  options = {
    path = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = "List of paths to include in the Bazel environment.";
    };
  };
}
