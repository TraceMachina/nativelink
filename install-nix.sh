#!/bin/sh

# This script installs the Nix package manager on your system by
# downloading a binary distribution and running its installer script
# (which in turn creates and populates /nix).

{ # Prevent execution if this script was only partially downloaded
oops() {
    echo "$0:" "$@" >&2
    exit 1
}

umask 0022

tmpDir="$(mktemp -d -t nix-binary-tarball-unpack.XXXXXXXXXX || \
          oops "Can't create temporary directory for downloading the Nix binary tarball")"
cleanup() {
    rm -rf "$tmpDir"
}
trap cleanup EXIT INT QUIT TERM

require_util() {
    command -v "$1" > /dev/null 2>&1 ||
        oops "you do not have '$1' installed, which I need to $2"
}

case "$(uname -s).$(uname -m)" in
    Linux.x86_64)
        hash=8808777ea0fc231bf131450bea32416f25eff49ea3ed025186e283ca6d4c5bba
        path=zvzwa03420whc67qm67xyc8klazsgqrz/nix-2.28.2-x86_64-linux.tar.xz
        system=x86_64-linux
        ;;
    Linux.i?86)
        hash=ff27a4f2a724e164fe49e60adcca6f6170403c2b4abe390db569e042c3d0d50c
        path=zp5zgsgs9yhm1pagv4nxfypzxns2zrvf/nix-2.28.2-i686-linux.tar.xz
        system=i686-linux
        ;;
    Linux.aarch64)
        hash=33674a4b5b6c213e638e99b4be7e6f005a9b2569297623e26763f6a0b746d503
        path=if58912zzr46k6rib5yay9q96xx2qsrg/nix-2.28.2-aarch64-linux.tar.xz
        system=aarch64-linux
        ;;
    Linux.armv6l)
        hash=fcb2c0733bf27c48b2741e6aabbc079b2787737daa14a83213f032f182ee6a0c
        path=glsybh1ri9j6a6z5g3ylwk5gafdamxpv/nix-2.28.2-armv6l-linux.tar.xz
        system=armv6l-linux
        ;;
    Linux.armv7l)
        hash=11a412d408ff61f1fe8aaed0b1b661399edc6419d0fc76b357ef9bf273797dca
        path=wijpnlhwlj4f01v3alnhqdzb7r4xlzlk/nix-2.28.2-armv7l-linux.tar.xz
        system=armv7l-linux
        ;;
    Linux.riscv64)
        hash=799ab5ba6e5543f53eaa69fac00ebf19a5ebb824cc9c4d15a0a965b3034230f7
        path=2zclpkmvhxcpqqxyl9m0dbnq4s49znmq/nix-2.28.2-riscv64-linux.tar.xz
        system=riscv64-linux
        ;;
    Darwin.x86_64)
        hash=49a06a56f1f9f1e6eea0b1c86587508b1b610b2ad9a111d96a79e33158e28d6f
        path=ijb0w8q43nf0qyfdszjgmi2lly4fbnlq/nix-2.28.2-x86_64-darwin.tar.xz
        system=x86_64-darwin
        ;;
    Darwin.arm64|Darwin.aarch64)
        hash=0687f4f4e0ffd479f3dcc3616820a22a6eac19faa250b90b5cbdfb16377adddb
        path=4swh4f0mglsd2r5hc4gxl4ng6vks1jgn/nix-2.28.2-aarch64-darwin.tar.xz
        system=aarch64-darwin
        ;;
    *) oops "sorry, there is no binary distribution of Nix for your platform";;
esac

# Use this command-line option to fetch the tarballs using nar-serve or Cachix
if [ "${1:-}" = "--tarball-url-prefix" ]; then
    if [ -z "${2:-}" ]; then
        oops "missing argument for --tarball-url-prefix"
    fi
    url=${2}/${path}
    shift 2
else
    url=https://releases.nixos.org/nix/nix-2.28.2/nix-2.28.2-$system.tar.xz
fi

tarball=$tmpDir/nix-2.28.2-$system.tar.xz

require_util tar "unpack the binary tarball"
if [ "$(uname -s)" != "Darwin" ]; then
    require_util xz "unpack the binary tarball"
fi

if command -v curl > /dev/null 2>&1; then
    fetch() { curl --fail -L "$1" -o "$2"; }
elif command -v wget > /dev/null 2>&1; then
    fetch() { wget "$1" -O "$2"; }
else
    oops "you don't have wget or curl installed, which I need to download the binary tarball"
fi

echo "downloading Nix 2.28.2 binary tarball for $system from '$url' to '$tmpDir'..."
fetch "$url" "$tarball" || oops "failed to download '$url'"

if command -v sha256sum > /dev/null 2>&1; then
    hash2="$(sha256sum -b "$tarball" | cut -c1-64)"
elif command -v shasum > /dev/null 2>&1; then
    hash2="$(shasum -a 256 -b "$tarball" | cut -c1-64)"
elif command -v openssl > /dev/null 2>&1; then
    hash2="$(openssl dgst -r -sha256 "$tarball" | cut -c1-64)"
else
    oops "cannot verify the SHA-256 hash of '$url'; you need one of 'shasum', 'sha256sum', or 'openssl'"
fi

if [ "$hash" != "$hash2" ]; then
    oops "SHA-256 hash mismatch in '$url'; expected $hash, got $hash2"
fi

unpack=$tmpDir/unpack
mkdir -p "$unpack"
tar -xJf "$tarball" -C "$unpack" || oops "failed to unpack '$url'"

script=$(echo "$unpack"/*/install)

[ -e "$script" ] || oops "installation script is missing from the binary tarball!"
export INVOKED_FROM_INSTALL_IN=1
"$script" "$@"

} # End of wrapping
