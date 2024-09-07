# Security policy

GitHub's bots crawl `nativelink` to detect security vulnerabilities wherever
possible.

TraceMachina and the `nativelink` authors place a high emphasis on fixing any
vulnerabilities. Please send a report if something doesn't look right.

## Supported versions

At the moment no version of `nativelink` is officially supported. Consider
using the latest commit on the `main` branch until official production binaries
are released.

## Reporting a vulnerability

Prefer reporting vulnerabilities via [GitHub](https://github.com/TraceMachina/nativelink/security).

<!-- vale off -->
If you'd rather communicate via email please contact <blaise@tracemachina.com>,
<marcus@tracemachina.com>, <blake@tracemachina.com> or <aaron@tracemachina.com>.
<!-- vale on -->

## Vulnerability disclosure and advisories

See [Advisories](https://github.com/TraceMachina/nativelink/security/advisories)
for publicly disclosed vulnerabilities.

## Using OCI Images

See the published [OCI images](https://github.com/TraceMachina/nativelink/pkgs/container/nativelink)
for pull commands.

Images are tagged by nix derivation hash. The most recently pushed image
corresponds to the `main` branch. Images are signed by the GitHub action that
produced the image. Note that the [OCI workflow](https://github.com/TraceMachina/nativelink/actions/workflows/image.yaml) might take a few minutes to publish the latest image.

### Get the tag for the latest commit

```sh
export LATEST=$(nix eval github:TraceMachina/nativelink#image.imageTag --raw)
```

### Verify the signature

```sh
cosign verify ghcr.io/tracemachina/nativelink:${LATEST} \
    --certificate-identity=https://github.com/TraceMachina/nativelink/.github/workflows/image.yaml@refs/heads/main \
    --certificate-oidc-issuer=https://token.actions.githubusercontent.com
```

### Get the Tag for a Specific Commit

For use in production pin the image to a specific revision:

```sh
# Get the tag for a specific commit
export PINNED_TAG=$(nix eval github:TraceMachina/nativelink/<revision>#image.imageTag --raw)
```

> [!TIP]
> The images are reproducible on `X86_64-unknown-linux-gnu`. If you're on such a
> system you can produce a binary-identical image by building the `.#image`
> flake output locally. Make sure that your `git status` is completely clean and
> aligned with the commit you want to reproduce. Otherwise the image will be
> tainted with a `"dirty"` revision label.
