# NativeLink Kubernetes deployments

Building blocks for NativeLink Kubernetes deployments.

This directory does **not** contain a one-size-fits-all solution like a Helm
chart - infrastructure requirements are too diverse for a single setup to
reliably cover all potential use-cases.

Instead, we provide useful building blocks in the form of Kustomizations.
Downstream implementers might use them as reference points to patch in the
functionality they require.

See the `deployment-examples` directory for concrete example deployments.
