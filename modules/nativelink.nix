{lib, ...}: {
  options = {
    endpoint = lib.mkOption {
      type = lib.types.str;
      description = lib.mdDoc ''
        The NativeLink Cloud endpoint.

        Defaults to NativeLink's shared cache.
      '';
      default = "grpcs://cas-tracemachina-shared.build-faster.nativelink.net";
    };
    api-key = lib.mkOption {
      type = lib.types.str;
      description = lib.mdDoc ''
        The API key to connect to the NativeLink Cloud.

        You should only use read-only keys here to prevent cache-poisoning and
        malicious artifact extractions.

        Defaults to NativeLink's shared read-only api key.
      '';
      default = "065f02f53f26a12331d5cfd00a778fb243bfb4e857b8fcd4c99273edfb15deae";
    };
    prefix = lib.mkOption {
      type = lib.types.str;
      description = lib.mdDoc ''
        An optional Bazel config prefix for the flags in `nativelink.bazelrc`.

        If set, builds need to explicitly enable the nativelink config via
        `--config=<prefix>`.

        Defaults to an empty string, enabling the cache by default.
      '';
      default = "";
    };
  };
}
