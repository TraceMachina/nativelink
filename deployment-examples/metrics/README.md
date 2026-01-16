# NativeLink Metrics with OpenTelemetry

This directory contains configurations and examples for collecting, processing, and visualizing NativeLink metrics using OpenTelemetry (OTEL) and various server systems.

## Overview

NativeLink exposes comprehensive metrics about cache operations and remote execution through OpenTelemetry. These metrics provide insights into:

- **Cache Performance**: Hit rates, operation latencies, eviction rates
- **Execution Pipeline**: Queue times, stage durations, success rates
- **System Health**: Worker utilization, throughput, error rates

## Quick Start

### Using Docker Compose (Recommended for Development)

1. Start the metrics stack:
```bash
cd deployment-examples/metrics
docker-compose up -d
```

2. Configure NativeLink to send metrics to the collector:
```bash
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
export OTEL_EXPORTER_OTLP_PROTOCOL=grpc
export OTEL_SERVICE_NAME=nativelink
export OTEL_RESOURCE_ATTRIBUTES="deployment.environment=dev,nativelink.instance_name=main"
```

3. Start NativeLink with your configuration:
```bash
nativelink /path/to/config.json
```

4. Access the metrics:
- Prometheus UI: http://localhost:9090
- Grafana: http://localhost:3000 (if included)
- OTEL Collector metrics: http://localhost:8888/metrics

### Using Kubernetes

1. Deploy the OTEL Collector:
```bash
kubectl apply -f kubernetes/otel-collector.yaml
```

2. Deploy Prometheus with OTLP receiver enabled:
```bash
kubectl apply -f kubernetes/prometheus.yaml
```

3. Configure NativeLink deployment to send metrics:
```yaml
env:
  - name: OTEL_EXPORTER_OTLP_ENDPOINT
    value: "http://otel-collector:4317"
  - name: OTEL_EXPORTER_OTLP_PROTOCOL
    value: "grpc"
  - name: OTEL_RESOURCE_ATTRIBUTES
    value: "deployment.environment=prod,k8s.cluster.name=main"
```

## Metrics Catalog

### Cache Metrics

| Metric | Type | Description | Labels |
|--------|------|-------------|--------|
| `nativelink_cache_operations_total` | Counter | Total cache operations | `cache_type`, `cache_operation_name`, `cache_operation_result` |
| `nativelink_cache_operation_duration` | Histogram | Operation latency in milliseconds | `cache_type`, `cache_operation_name` |
| `nativelink_cache_io_total` | Counter | Bytes read/written | `cache_type`, `cache_operation_name` |
| `nativelink_cache_size` | Gauge | Current cache size in bytes | `cache_type` |
| `nativelink_cache_entries` | Gauge | Number of cached entries | `cache_type` |
| `nativelink_cache_item_size` | Histogram | Size distribution of cache entries | `cache_type` |

**Cache Operation Names:**
- `read`: Data retrieval operations
- `write`: Data storage operations
- `delete`: Explicit removal operations
- `evict`: Automatic evictions (LRU, TTL)

**Cache Operation Results:**
- `hit`: Data found and valid (reads)
- `miss`: Data not found (reads)
- `expired`: Data found but stale (reads)
- `success`: Operation completed (writes/deletes)
- `error`: Operation failed

### Execution Metrics

| Metric | Type | Description | Labels |
|--------|------|-------------|--------|
| `nativelink_execution_stage_duration` | Histogram | Time spent in each execution stage | `execution_stage` |
| `nativelink_execution_total_duration` | Histogram | Total execution time from submission to completion | `execution_instance` |
| `nativelink_execution_queue_time` | Histogram | Time spent waiting in queue | `execution_priority` |
| `nativelink_execution_active_count` | Gauge | Current actions in each stage | `execution_stage` |
| `nativelink_execution_completed_count_total` | Counter | Completed executions | `execution_result`, `execution_action_digest` |
| `nativelink_execution_stage_transitions_total` | Counter | Stage transition events | `execution_instance`, `execution_priority` |
| `nativelink_execution_output_size` | Histogram | Size of execution outputs | - |
| `nativelink_execution_retry_count_total` | Counter | Number of retries | - |

**Execution Stages:**
- `unknown`: Initial state
- `cache_check`: Checking for cached results
- `queued`: Waiting for available worker
- `executing`: Running on worker
- `completed`: Finished execution

**Execution Results:**
- `success`: Completed with exit code 0
- `failure`: Completed with non-zero exit code
- `cancelled`: Execution was cancelled
- `timeout`: Execution timed out
- `cache_hit`: Result found in cache

> **Note on Counter Names in Prometheus:** Counter metrics are exposed with a `_total` suffix
> (for example, `nativelink_execution_completed_count_total`). The Docker Compose quickstart,
> recording rules, and included dashboards assume `_total` counter names.

## Configuration

### Environment Variables

NativeLink uses standard OpenTelemetry environment variables:

```bash
# OTLP Exporter Configuration
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317  # Collector endpoint
OTEL_EXPORTER_OTLP_PROTOCOL=grpc                   # Protocol (grpc or http/protobuf)
OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer token"  # Optional auth headers
OTEL_EXPORTER_OTLP_COMPRESSION=gzip                # Compression (none, gzip)

# Resource Attributes
OTEL_SERVICE_NAME=nativelink                       # Service name (fixed)
OTEL_RESOURCE_ATTRIBUTES="key1=value1,key2=value2" # Custom attributes

# Metric Export Configuration
OTEL_METRIC_EXPORT_INTERVAL=60000                  # Export interval in ms (default: 60s)
OTEL_METRIC_EXPORT_TIMEOUT=30000                   # Export timeout in ms (default: 30s)

# Disable telemetry types
OTEL_TRACES_EXPORTER=none                          # Disable traces (if only metrics needed)
OTEL_LOGS_EXPORTER=none                            # Disable logs (if only metrics needed)
```

### Collector Configuration

The OTEL Collector can be configured to:
1. Add resource attributes
2. Batch metrics for efficiency
3. Export to multiple metrics servers
4. Transform metric attributes

See `otel-collector-config.yaml` for a complete example.

## Server Options

### Prometheus (Recommended)

Prometheus offers native OTLP support and excellent query capabilities.

**Direct OTLP Ingestion:**
```bash
prometheus --web.enable-otlp-receiver \
          --storage.tsdb.out-of-order-time-window=30m
```

**Via Collector Scraping:**
```yaml
scrape_configs:
  - job_name: 'otel-collector'
    static_configs:
      - targets: ['otel-collector:9090']
```

### Grafana Cloud

For managed metrics:
```yaml
exporters:
  otlphttp:
    endpoint: https://otlp-gateway-prod-us-central-0.grafana.net/otlp
    headers:
      Authorization: "Bearer ${GRAFANA_CLOUD_TOKEN}"
```

### ClickHouse

For high-volume metrics storage:
```yaml
exporters:
  clickhouse:
    endpoint: tcp://clickhouse:9000
    database: metrics
    ttl_days: 30
    logs_table: otel_logs
    metrics_table: otel_metrics
```

### Quickwit

For unified logs and metrics:
```yaml
exporters:
  otlp:
    endpoint: quickwit:7281
    headers:
      "x-quickwit-index": "nativelink-metrics"
```

## Example Queries

### Prometheus/PromQL

**Cache hit rate:**
```promql
sum(rate(nativelink_cache_operations_total{cache_operation_result="hit"}[5m])) by (cache_type) /
sum(rate(nativelink_cache_operations_total{cache_operation_name="read"}[5m])) by (cache_type)
```

**Execution success rate:**
```promql
sum(rate(nativelink_execution_completed_count_total{execution_result="success"}[5m])) /
sum(rate(nativelink_execution_completed_count_total[5m]))
```

**Queue depth by priority:**
```promql
sum(nativelink_execution_active_count{execution_stage="queued"}) by (execution_priority)
```

**P95 cache operation latency:**
```promql
histogram_quantile(0.95,
  sum(rate(nativelink_cache_operation_duration_bucket[5m])) by (le, cache_type)
)
```

**Worker utilization:**
```promql
count(nativelink_execution_active_count{execution_stage="executing"} > 0) /
count(count by (execution_worker_id) (nativelink_execution_active_count))
```

### Joining with Resource Attributes

Use `target_info` to join resource attributes:
```promql
rate(nativelink_execution_completed_count_total[5m])
* on (job, instance) group_left (k8s_cluster_name, deployment_environment)
target_info
```

## Dashboards

### Grafana Dashboard

Import the included dashboard for a comprehensive view:
```bash
# Import via API
curl -X POST http://admin:admin@localhost:3000/api/dashboards/db \
  -H "Content-Type: application/json" \
  -d @grafana-dashboard.json

# Or import via UI at http://localhost:3000
```

Key panels include:
- Execution pipeline overview
- Cache performance metrics
- Worker utilization heatmap
- Error rate tracking
- Queue depth over time
- Stage duration percentiles

## Alerting

### Example Alert Rules

```yaml
groups:
  - name: nativelink_alerts
    rules:
      - alert: HighErrorRate
        expr: |
          (1 - (
            sum(rate(nativelink_execution_completed_count_total{execution_result="success"}[5m])) /
            sum(rate(nativelink_execution_completed_count_total[5m]))
          )) > 0.05
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "High execution error rate ({{ $value | humanizePercentage }})"

      - alert: CacheMissRateHigh
        expr: |
          (1 - nativelink:cache_hit_rate) > 0.5
        for: 10m
        labels:
          severity: info
        annotations:
          summary: "Cache miss rate above 50% for {{ $labels.cache_type }}"

      - alert: QueueBacklog
        expr: |
          sum(nativelink_execution_active_count{execution_stage="queued"}) > 100
        for: 15m
        labels:
          severity: warning
        annotations:
          summary: "Queue backlog above 100 actions"

      - alert: WorkerUtilizationLow
        expr: |
          nativelink:worker_utilization < 0.3
        for: 30m
        labels:
          severity: info
        annotations:
          summary: "Worker utilization below 30%"
```

## Troubleshooting

### No Metrics Appearing

1. Check NativeLink is configured with OTEL environment variables:
```bash
ps aux | grep nativelink | grep OTEL
```

2. Verify collector is receiving data:
```bash
curl http://localhost:13133/health
curl http://localhost:8888/metrics | grep otelcol_receiver_accepted_metric_points
```

3. Check collector logs:
```bash
docker logs otel-collector
# or
kubectl logs -l app=otel-collector
```

### Cache Metrics Missing

If you see `nativelink_execution_*` metrics but no `nativelink_cache_*` metrics, your NativeLink build may not be emitting store-level cache operation metrics yet. In that case, cache recording rules like `nativelink:cache_hit_rate` won't produce any series.

### High Memory Usage

1. Adjust collector batch size:
```yaml
processors:
  batch:
    send_batch_size: 512  # Reduce from 1024
```

2. Increase memory limits:
```yaml
memory_limiter:
  limit_mib: 1024  # Increase from 512
```

3. Reduce metric cardinality by dropping labels:
```yaml
processors:
  attributes:
    actions:
      - key: unnecessary_label
        action: delete
```

### Out-of-Order Samples

Enable out-of-order ingestion in Prometheus:
```yaml
storage:
  tsdb:
    out_of_order_time_window: 1h  # Increase from 30m
```

### Missing Resource Attributes

Ensure attributes are promoted in Prometheus:
```yaml
otlp:
  promote_resource_attributes:
    - your.custom.attribute
```

## Performance Tuning

### Collector Optimization

1. **Batching**: Adjust batch processor settings based on volume
2. **Compression**: Enable gzip for network efficiency
3. **Sampling**: Use tail sampling for high-volume traces
4. **Filtering**: Drop unnecessary metrics at collector level

### Prometheus Optimization

1. **Recording Rules**: Pre-calculate expensive queries
2. **Retention**: Set appropriate retention periods
3. **Downsampling**: Use Thanos or Cortex for long-term storage
4. **Federation**: Split metrics across multiple Prometheus instances

### NativeLink Optimization

1. **Export Interval**: Increase `OTEL_METRIC_EXPORT_INTERVAL` to reduce overhead
2. **Resource Attributes**: Minimize cardinality of custom attributes
3. **Metric Selection**: Disable unused metric types if needed

## Additional Resources

- [OpenTelemetry Documentation](https://opentelemetry.io/docs/)
- [Prometheus Best Practices](https://prometheus.io/docs/practices/)
- [OTEL Collector Configuration](https://opentelemetry.io/docs/collector/configuration/)
- [NativeLink Documentation](https://nativelink.com/docs)
- [Grafana Dashboard Examples](https://grafana.com/grafana/dashboards/)

## Support

For issues or questions:
- File an issue: https://github.com/TraceMachina/nativelink/issues
- Join our Discord: https://discord.gg/nativelink
- Check documentation: https://nativelink.com/docs
