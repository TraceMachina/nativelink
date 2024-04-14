# Contributing to NativeLink

NativeLink welcomes contribution from everyone. Here are the guidelines if you
are thinking of helping us:

## Contributions

Contributions to NativeLink or its dependencies should be made in the form of
GitHub pull requests. Each pull request will be reviewed by a core contributor
(someone with permission to land patches) and either landed in the main tree or
given feedback for changes that would be required. All contributions should
follow this format, even those from core contributors.

Should you wish to work on an issue, please claim it first by commenting on
the GitHub issue that you want to work on it. This is to prevent duplicated
efforts from contributors on the same issue.

## Nix development flake

You can use the Nix development flake to automatically set up Bazel, Cargo and
various Cloud tools for you:

1. Install the [nix package manger](https://nixos.org/download.html) and enable
   [flakes](https://nixos.wiki/wiki/Flakes).
2. Optionally, install [`direnv`](https://direnv.net/docs/installation.html) and
   hook it into your shell.
3. We currently don't ship a C++ toolchain as part of the flake. Make sure to
   install a recent version of Clang.

## Pull Request Checklist

- Branch from the main branch and, if needed, rebase to the current main
  branch before submitting your pull request. If it doesn't merge cleanly with
  main you may be asked to rebase your changes.

- Commits should be as small as possible, while ensuring that each commit is
  correct independently (that is, each commit should compile and pass tests).

- Commits should be accompanied by a [Developer Certificate of Origin](http://developercertificate.org)
  sign-off, which indicates that you (and your employer if applicable) agree to
  be bound by the terms of the [project license](LICENSE). In git, this is the
  `-s` option to `git commit`.

- If your patch isn't getting reviewed or you need a specific person to review
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

  For Windows PowerShell;

  ```powershell
  bazel run `
    --@rules_rust//:rustfmt.toml=//:.rustfmt.toml `
    @rules_rust//:rustfmt
  ```

## Writing documentation

NativeLink largely follows the [Microsoft Style Guide](https://learn.microsoft.com/en-us/style-guide/welcome/).

NativeLink implements it's documentation style guide via Vale. The pre-commit
hooks forbid errors but permit warnings and suggestions. To view all of Vale's
suggestions invoke it directly:

```
vale somefile
```

## Creating releases

To keep the release process in line with best practices for open source
repositories, not all steps are automated. Specifically, tags should be signed
and pushed manually and the release notes should be human readable beyond what
most automatically generated changelogs provide.

1. Bump the current version in the following files:

   - `flake.nix`
   - `MODULE.bazel`
   - `Cargo.toml`
   - `nativelink-*/Cargo.toml`
   - `nativelink-docs/package.json`

2. Run `git cliff --tag=0.x.y > CHANGELOG.md` to update the changelog. You might
   need to make manual adjustments to `cliff.toml` if `git-cliff` doesn't put a
   commit in the right subsection.

3. Create the commit and PR. Call it `Release NativeLink v0.x.y`.

4. Once the PR is merged, update your local repository and origin:

   ```bash
   git switch main
   git pull -r upstream main
   git push
   ```

5. Create a **signed** tag on the release commit and give it the same tag
   message as the name of the tag. This tag should be the version number with a
   `v` prefix:

   ```bash
   git tag -s v0.x.y

   # tag message should be: v0.x.y
   ```

6. Push the signed tag to the origin repository:

   ```bash
   git push origin v0.x.y
   ```

7. Pushing the tag triggers an additional GHA workflow which should create the
   container images in your own fork. Check that this workflow is functional. If
   the CI job in your fork passes, push the tag to upstream:

   ```bash
   git push upstream v0.x.y
   ```

8. The images for the release are now being created. Go to the [Tags](https://github.com/TraceMachina/nativelink/tags)
   tab in GitHub and double-check that the tag has a green `Verified` marker
   next to it. If it does, select `Create a release from tag` and create release
   notes. You can use previous release notes as template by clicking on the
   "Edit" button on a previous release and copy-pasting the contents into the
   new release notes.

   Make sure to include migration instructions for all breaking changes.

   Explicitly list whatever changes you think are worth mentioning as `Major
   changes`. This is a fairly free-form section that doesn't have any explicit
   requirements other than being a best-effort summary of notable changes.

9. Once all notes are in line, click `Publish Release`.

## Conduct

NativeLink Code of Conduct is available in the
[CODE_OF_CONDUCT](CODE_OF_CONDUCT.md) file.
