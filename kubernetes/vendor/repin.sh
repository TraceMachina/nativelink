#!/usr/bin/env bash

# Tekton Operator
wget \
    https://storage.googleapis.com/tekton-releases/operator/previous/v0.75.0/release.yaml \
    -O tekton-operator/tekton-operator.yaml

# Operator Lifecycle Manager
wget \
    https://github.com/operator-framework/operator-lifecycle-manager/releases/download/v0.31.0/crds.yaml \
    -O olm/crds.yaml
wget \
    https://github.com/operator-framework/operator-lifecycle-manager/releases/download/v0.31.0/olm.yaml \
    -O olm/olm.yaml

# OpenTelemetry
wget \
    https://operatorhub.io/install/opentelemetry-operator.yaml \
    -O operators/opentelemetry-operator.yaml

# Grafana operator
wget \
    https://operatorhub.io/install/grafana-operator.yaml \
    -O operators/grafana-operator.yaml

# VictoriaMetrics and VictoriaLogs
wget \
    https://operatorhub.io/install/victoriametrics-operator.yaml \
    -O operators/victoriametrics-operator.yaml

# Tempo
wget \
    https://operatorhub.io/install/tempo-operator.yaml \
    -O operators/tempo-operator.yaml
