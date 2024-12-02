_final: prev: {
  pulumi = prev.pulumi.overrideAttrs (_old: {
    version = "3.137.0";
    src = prev.fetchFromGitHub {
      owner = "pulumi";
      repo = "pulumi";
      rev = "v3.137.0";
      hash = "sha256-U1BubJIqQu+tAYQz8ojVrJpx6ZbMFEarSCmmuahG6ys=";
      name = "pulumi"; # Some tests rely on checkout directory name
    };
    vendorHash = "sha256-Q9NTlgd+rOD5vmbmeAepIARvFtNQT/IFLPeaFC74E7c=";

    # Remove old patch that's no longer needed
    patches = [];

    preCheck =
      ''
        # The tests require `version.Version` to be unset
        ldflags=''${ldflags//"$importpathFlags"/}
        # Create some placeholders for plugins used in tests
        for name in pulumi-{resource-pkg{A,B},-pkgB}; do
          ln -s ${prev.coreutils}/bin/true "$name"
        done
        # Code generation tests also download dependencies from network
        rm codegen/{docs,dotnet,go,nodejs,python,schema}/*_test.go
        # Azure tests are not being skipped when an Azure token is not provided
        rm secrets/cloud/azure_test.go
        rm -R codegen/{dotnet,go,nodejs,python}/gen_program_test
        unset PULUMI_ACCESS_TOKEN
        export PULUMI_DISABLE_AUTOMATIC_PLUGIN_ACQUISITION=true
      ''
      + prev.lib.optionalString prev.stdenv.hostPlatform.isDarwin ''
        export PULUMI_HOME=$(mktemp -d)
      '';

    doCheck = false;
  });

  pulumiPackages =
    prev.pulumiPackages
    // {
      pulumi-language-go = prev.pulumiPackages.pulumi-language-go.overrideAttrs (_old: {
        vendorHash = "sha256-Bk/JxFYBnd+bOlExJKGMIl2oUHPg3ViIVBqKBojoEm4=";
      });
    };
}
