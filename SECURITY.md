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

If you'd rather communicate via email please contact <blaise@tracemachina.com>,
<marcus@tracemachina.com>, <blake@tracemachina.com> or <aaron@tracemachina.com>.

## Vulnerability disclosure and advisories

See [Advisories](https://github.com/TraceMachina/nativelink/security/advisories)
for publicly disclosed vulnerabilities.

## Using OCI Images

See the published [OCI images](https://github.com/TraceMachina/nativelink/pkgs/container/nativelink)
for pull commands.

Images are tagged by nix derivation hash. The most recently pushed image
corresponds to the `main` branch. Images are signed by the GitHub action that
produced the image. Note that the [OCI workflow](https://github.com/TraceMachina/nativelink/actions/workflows/image.yaml) might take a few minutes to publish the latest image.
