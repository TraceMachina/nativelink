## Setup
Install bazel 3.0.0+ then run:
```
$ bazel build //...
```

## How to update external rust deps
Install `cargo` and then run: `cargo install cargo-raze`.
From now on you can use: 
```
$ cargo generate-lockfile  # This will generate a new Cargo.lock file.
$ cargo raze  # This will code-gen the bazel rules.
```

Then test your changes.
