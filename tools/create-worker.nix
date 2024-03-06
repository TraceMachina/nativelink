{
  pkgs,
  nativelink,
  buildImage,
  ...
}: let
  # A temporary directory. Note that this doesn't set any permissions. Those
  # need to be added explicitly in the final image arguments.
  mkTmp = pkgs.runCommand "mkTmp" {} ''
    mkdir -p $out/tmp
  '';

  # Permissions for the temporary directory.
  mkTmpPerms = {
    path = mkTmp;
    regex = ".*";
    mode = "1777";
    uid = 0; # Owned by root.
    gid = 0; # Owned by root.
  };

  # Enable the shebang `#!/usr/bin/env bash`.
  mkEnvSymlink = pkgs.runCommand "mkEnvSymlink" {} ''
    mkdir -p $out/usr/bin
    ln -s /bin/env $out/usr/bin/env
  '';
in
  # Create a container image from a base image with the nativelink executable
  # added and set as entrypoint. This allows arbitrary base images to be
  # "enriched" with nativelink to create worker images for cloud deployments.
  image:
    buildImage {
      name = "nativelink-worker-${image.imageName}";
      fromImage = image;
      maxLayers = 20;
      copyToRoot = [
        mkTmp
        mkEnvSymlink
        (pkgs.buildEnv {
          name = "${image.imageName}-buildEnv";
          paths = [nativelink pkgs.coreutils pkgs.bash];
          pathsToLink = ["/bin"];
        })
      ];
      perms = [mkTmpPerms];

      # Override the final image tag with the one from the base image. This way
      # the nativelink executable doesn't influence this tag and and changes to
      # its codebase don't invalidate existing toolchain containers.
      tag = image.imageTag;
    }
