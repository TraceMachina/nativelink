# End-to-end check for the Nix binary-cache (substituter) facade.
#
# Boots a NixOS VM running `nativelink` with two `nix_cache` services and
# drives a real `nix` client through the full round trip:
#
#   1. `/nix-cache-info` answers 200 and advertises the configured store dir.
#   2. A throwaway store path is pushed with `nix copy --to` (uncompressed).
#   3. The served `.narinfo` names the right path, is `Compression: none`, and
#      carries a `Sig:` line from the server's signing key.
#   4. The path is substituted back with `nix copy --from` under signature
#      checking — proving the served signature verifies against the key.
#   5. Read-through: a second instance ('front') configured with the first as
#      its upstream serves a path only the upstream has, fetching + verifying +
#      caching it on first request, then serving it from its OWN stores.
#
# This is the first NixOS VM test in the repo; it also seeds the VM-check
# infrastructure the provisioning appendix (Appendix E) relies on. Linux-only:
# `runNixOSTest` needs KVM.
{
  pkgs,
  nativelink,
}: let
  port = 50071;
  frontPort = 50072;
  cacheUrl = "http://localhost:${toString port}/nix/main";
  frontUrl = "http://localhost:${toString frontPort}/nix/front";

  # Throwaway ed25519 signing key, shared with the Rust integration suite
  # (`nativelink-service/tests/nix_cache_server_test.rs`). It protects nothing.
  serverSecretKey = "test-int-1:59Az40GGu2M3sM6rQ6T/+c61OMLw1r+qx9xMe/SzkIkjJupCgoxM63utsPNUImg3vc6stSSfAlXYYgmmUSZ5Ig==";
  # `nix key convert-secret-to-public` of the secret above.
  serverPublicKey = "test-int-1:IybqQoKMTOt7rbDzVCJoN73OrLUknwJV2GIJplEmeSI=";

  # A verify-wrapped fast/slow NAR store, a completeness_checking path-info
  # store, and a plain alias store, rooted under `dir` — the composition
  # documented in nativelink-config/examples/nix_cache.json5. `tag` keeps the
  # two instances' store names distinct.
  storeSet = tag: dir: [
    {
      name = "${tag}_NAR_STORE";
      verify = {
        verify_size = true;
        verify_hash = true;
        backend.fast_slow = {
          fast.filesystem = {
            content_path = "${dir}/nar-fast/content";
            temp_path = "${dir}/nar-fast/temp";
            eviction_policy.max_bytes = 1000000000;
          };
          slow.filesystem = {
            content_path = "${dir}/nar-slow/content";
            temp_path = "${dir}/nar-slow/temp";
            eviction_policy.max_bytes = 50000000000;
          };
        };
      };
    }
    {
      name = "${tag}_PATH_INFO_STORE";
      completeness_checking = {
        backend.memory.eviction_policy.max_bytes = 100000000;
        cas_store.ref_store.name = "${tag}_NAR_STORE";
      };
    }
    {
      name = "${tag}_ALIAS_STORE";
      memory.eviction_policy.max_bytes = 100000000;
    }
  ];

  # Two instances in one process: `main` is populated directly by `nix copy`;
  # `front` holds nothing itself and read-throughs to `main`. JSON is a subset
  # of JSON5, so this loads as-is.
  nixCacheConfig = pkgs.writeText "nix_cache.json5" (builtins.toJSON {
    stores = storeSet "UP" "/var/lib/nativelink/up" ++ storeSet "FRONT" "/var/lib/nativelink/front";
    servers = [
      {
        name = "nix_cache_main";
        listener.http.socket_address = "0.0.0.0:${toString port}";
        services = {
          nix_cache = [
            {
              instance_name = "main";
              cas_store = "UP_NAR_STORE";
              path_info_store = "UP_PATH_INFO_STORE";
              alias_store = "UP_ALIAS_STORE";
              store_dir = "/nix/store";
              priority = 40;
              want_mass_query = true;
              signing_key_files = ["/etc/nativelink/nix-cache.key"];
              read_only = false;
            }
          ];
          health = {};
        };
      }
      {
        name = "nix_cache_front";
        listener.http.socket_address = "0.0.0.0:${toString frontPort}";
        services = {
          nix_cache = [
            {
              instance_name = "front";
              cas_store = "FRONT_NAR_STORE";
              path_info_store = "FRONT_PATH_INFO_STORE";
              alias_store = "FRONT_ALIAS_STORE";
              store_dir = "/nix/store";
              priority = 30;
              want_mass_query = true;
              signing_key_files = ["/etc/nativelink/nix-cache.key"];
              read_only = false;
              upstream_caches = [
                {
                  url = cacheUrl;
                  trusted_public_keys = [serverPublicKey];
                }
              ];
            }
          ];
          health = {};
        };
      }
    ];
  });
in
  pkgs.testers.runNixOSTest {
    name = "nix-substituter-e2e";

    nodes.machine = {lib, ...}: {
      environment.systemPackages = [
        nativelink
        pkgs.curl
      ];

      # The server's signing key. A store-path leak is fine: it is throwaway.
      environment.etc."nativelink/nix-cache.key".text = serverSecretKey;

      systemd.services.nativelink-nix-cache = {
        description = "NativeLink Nix binary-cache facade (e2e test)";
        wantedBy = ["multi-user.target"];
        after = ["network.target"];
        serviceConfig = {
          ExecStart = "${nativelink}/bin/nativelink ${nixCacheConfig}";
          StateDirectory = "nativelink";
          Restart = "on-failure";
          RestartSec = "1s";
        };
      };

      # Trust the facade's key so a signature-checked pull verifies, and keep
      # the client off the network (the test names its source explicitly).
      nix.settings = {
        experimental-features = [
          "nix-command"
          "flakes"
        ];
        trusted-public-keys = [serverPublicKey];
        substituters = lib.mkForce [];
      };
    };

    testScript = ''
      machine.wait_for_unit("nativelink-nix-cache.service")
      machine.wait_for_open_port(${toString port})
      machine.wait_for_open_port(${toString frontPort})

      # 1. The existence probe must answer 200 and advertise our store dir.
      info = machine.succeed("curl -sf ${cacheUrl}/nix-cache-info")
      assert "StoreDir: /nix/store" in info, f"unexpected nix-cache-info: {info!r}"

      # 2. Push a throwaway store path uncompressed.
      machine.succeed("echo straylight-nix-substituter-e2e > /tmp/payload")
      storepath = machine.succeed("nix-store --add /tmp/payload").strip()
      hashpart = storepath.split("/")[3].split("-")[0]
      machine.succeed(f"nix copy --to '${cacheUrl}?compression=none' {storepath}")

      # 3. The served narinfo must name the right path, be uncompressed, and be
      #    signed by the server key.
      narinfo = machine.succeed(f"curl -sf ${cacheUrl}/{hashpart}.narinfo")
      assert f"StorePath: {storepath}" in narinfo, f"unexpected narinfo: {narinfo!r}"
      assert "Compression: none" in narinfo, f"expected uncompressed: {narinfo!r}"
      assert "Sig: test-int-1:" in narinfo, f"missing server signature: {narinfo!r}"

      # 4. Substitute the path back FROM the cache into a SEPARATE store root
      #    with signature checking on: proves the served Sig verifies against the
      #    advertised public key. Using a fresh root (rather than deleting from
      #    the main store) keeps the check fast — a main-store delete drags in a
      #    full store-optimise scan.
      machine.succeed(
          f"nix copy --from '${cacheUrl}' --to 'local?root=/tmp/dest' "
          f"--extra-trusted-public-keys '${serverPublicKey}' {storepath}"
      )
      machine.succeed(f"cmp /tmp/dest{storepath} /tmp/payload")

      # 5. Read-through: the 'front' instance holds nothing and has 'main' as its
      #    upstream. A path present only on 'main' is fetched, verified, and
      #    cached by 'front' on first request. Push a NEW path to 'main' only.
      machine.succeed("echo straylight-read-through-e2e > /tmp/payload2")
      rtpath = machine.succeed("nix-store --add /tmp/payload2").strip()
      rthash = rtpath.split("/")[3].split("-")[0]
      machine.succeed(f"nix copy --to '${cacheUrl}?compression=none' {rtpath}")

      # 'front' has never seen it. `nix copy --from` first probes availability
      # with a HEAD on the narinfo and then GETs it — both must read-through to
      # 'main' — so a signature-checked pull FROM 'front' succeeds end to end.
      machine.succeed(
          f"nix copy --from '${frontUrl}' --to 'local?root=/tmp/dest2' "
          f"--extra-trusted-public-keys '${serverPublicKey}' {rtpath}"
      )
      machine.succeed(f"cmp /tmp/dest2{rtpath} /tmp/payload2")

      # Durable: 'front' now serves the record from its OWN stores, re-signed
      # and verifiable.
      rt_narinfo = machine.succeed(f"curl -sf ${frontUrl}/{rthash}.narinfo")
      assert f"StorePath: {rtpath}" in rt_narinfo, f"front narinfo: {rt_narinfo!r}"
      assert "Sig: test-int-1:" in rt_narinfo, f"front narinfo missing sig: {rt_narinfo!r}"
    '';
  }
