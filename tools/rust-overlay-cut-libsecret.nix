final: _prev: {
  rust-overlay =
    final.rust-overlay
    // {
      mkComponent = args @ {lib, ...}:
        (final.rust-overlay.mkComponent args)
        // {
          postFixup =
            lib.replaceStrings
            ["patchelf --add-needed ${args.pkgsHostHost.libsecret}/lib/libsecret-1.so.0 $out/bin/cargo"]
            ["# patchelf --add-needed ${args.pkgsHostHost.libsecret}/lib/libsecret-1.so.0 $out/bin/cargo"]
            ((final.rust-overlay.mkComponent args).postFixup or "");
        };
    };
}
