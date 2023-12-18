# Contributing to Native Link

Native Link welcomes contribution from everyone. Here are the guidelines if you
are thinking of helping us:

## Contributions

Contributions to Native Link or its dependencies should be made in the form of
GitHub pull requests. Each pull request will be reviewed by a core contributor
(someone with permission to land patches) and either landed in the main tree or
given feedback for changes that would be required. All contributions should
follow this format, even those from core contributors.

Should you wish to work on an issue, please claim it first by commenting on
the GitHub issue that you want to work on it. This is to prevent duplicated
efforts from contributors on the same issue.

## Nix development flake

You can use the Nix development flake to automatically set up Bazel, Cargo and
various Cloud CLIs for you:

1. Install the [nix package manger](https://nixos.org/download.html) and enable
   [flakes](https://nixos.wiki/wiki/Flakes).
2. Optionally, install [direnv](https://direnv.net/docs/installation.html) and
   hook it into your shell.
3. We currently don't ship a C++ toolchain as part of the flake. Make sure to
   install a recent version of Clang.

## Pull Request Checklist

- Branch from the main branch and, if needed, rebase to the current main
  branch before submitting your pull request. If it doesn't merge cleanly with
  main you may be asked to rebase your changes.

- Commits should be as small as possible, while ensuring that each commit is
  correct independently (i.e., each commit should compile and pass tests).

- Commits should be accompanied by a [Developer Certificate of Origin](http://developercertificate.org)
  sign-off, which indicates that you (and your employer if applicable) agree to
  be bound by the terms of the [project license](LICENSE). In git, this is the
  `-s` option to `git commit`.

- If your patch is not getting reviewed or you need a specific person to review
  it, you can @-reply a reviewer asking for a review in the pull request or a
  comment.

- Add tests relevant to the fixed bug or new feature.

- If `rustfmt` complains you can use the following command to apply its
  suggested changes to the Rust sources:

  ```bash
  bazel run \
    --@rules_rust//:rustfmt.toml=//:.rustfmt.toml \
    @rules_rust//:rustfmt
  ```

  For Windows Powershell;

  ```powershell
  bazel run `
    --@rules_rust//:rustfmt.toml=//:.rustfmt.toml `
    @rules_rust//:rustfmt
  ```

## Updating Rust dependencies

After modifying the corresponding `Cargo.toml` file in either the top level or
one of the crate subdirectories run the following command to rebuild the crate
index:

```
CARGO_BAZEL_REPIN=1 bazel sync --only=crate_index
```

This updates `Cargo.lock` and `Cargo.Bazel.lock` with the new dependency
information.

## Conduct

Native Link Code of Conduct is available in the
[CODE_OF_CONDUCT](CODE_OF_CONDUCT.md) file.
