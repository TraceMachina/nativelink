# Reclient deployment

This example shows how to set up a Nativ Link to work with Reclient for
Chromium builds.  This configuration is an example that runs all of the
services on a single machine which is obviously pointless, however it is a
good starting point to allow you to move the CAS and Worker roles to other
machines.

## Roles

There are a number of roles required to perform a Reclient set up:

 - Native Link Scheduler - The Native Link instance that handles all of the
   communications from Reproxy.  This includes scheduling jobs on the workers
   and proxying the CAS requests.

 - Native Link CAS - The storage instance that stores all required build files
   and the build outputs.  This also acts as the AC (action cache) which stores
   the results of previous actions.

 - Native Link Worker - There can be many of these and they perform the build
   work.  They are rate limited to perform one build per core using the
   cpu_count property.  The worker_precondition_script.sh ensures that they have
   sufficient RAM to perform a build before it starts.

## Running

To start Native Link simply execute `docker-compose up` from this directory.
A new Native Link instance with all of the backing will be built and started on
port 50052.

Create the file buildtools/reclient_cfgs/reproxy.cfg with the following:
```
instance=main
service=127.0.0.1:50052
# Disables both TLS and authentication
service_no_security=true
# Required to stop autoninja from complaining about authentication despite being
# implied by service_no_security
service_no_auth=true
# Try not to use local execution, can't set to 0 otherwise it appears to ignore
local_resource_fraction=0.00001
log_format=reducedtext
automatic_auth=false
gcert_refresh_timeout=20
fail_early_min_action_count=4000
fail_early_min_fallback_ratio=0.5
deps_cache_max_mb=256
# TODO(b/276727504) Re-enable once noop build shutdown time bug is fixed
# enable_deps_cache=true
async_reproxy_termination=true
use_unified_uploads=true
fast_log_collection=true
depsscanner_address=exec://%s/buildtools/reclient/scandeps_server

# Improve upload/download concurrency
max_concurrent_streams_per_conn=50
max_concurrent_requests_per_conn=50
min_grpc_connections=50
cas_concurrency=1000

# Native Link doesn't currently support compressed blob upload
compression_threshold=-1
use_batches=false

# Metric metadata
metrics_namespace=main
```

Now that the environment is set up you simply need to enable it for your
Chromium build by modifying your `args.gn` file to contain:

```
use_remoteexec=true
rbe_cfg_dir="../../buildtools/reclient_cfgs/linux"
```

Alternately run:

```sh
gn gen --args="use_remoteexec=true rbe_cfg_dir=\"../../buildtools/reclient_cfgs/linux\"" out/Default
```

Now run your build using autoninja which will automatically start reproxy and
do all of your execution for you.

You should set a crontab to clean up stale docker images nightly on workers:
```sh
docker image prune -a --filter "until=24h" -f
```
