# https://github.com/aws/aws-lc-rs/blob/main/aws-lc-sys/README.md#using-a-system-installed-aws-lc
# assumes all the things are installed in one dir, which nix doesn't do
# This script builds said dir
{
  stdenv,
  aws-lc,
  aws-lc-dev,
}:
stdenv.mkDerivation {
  name = "aws-lc-system-dir";
  unpackPhase = "true";
  buildInputs = [aws-lc aws-lc-dev];
  buildPhase = ''
    mkdir ''${out}
    cp --no-preserve=mode,ownership -srt ''${out} ${aws-lc}/. ${aws-lc-dev}/.
  '';
}
