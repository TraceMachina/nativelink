# Reclient deployment

This example shows how to set up a Turbo-Cache to work with Reclient for
Chromium builds.  This configuration is an example that runs all of the
services on a single machine which is obviously pointless, however it is a
good starting point to allow you to move the CAS and Worker roles to other
machines.

## Roles

There a number of roles required to perform a Reclient set up:

 - Turbo Cache Scheduler - The Turbo Cache instance that communicates with the
   Goma proxy and handles scheduling of tasks to workers within the Turbo Cache
   cluster.  Because the Goma proxy is unable to talk to a separate scheduler
   and CAS endpoint this also proxies CAS and AC requests to the storage
   instance.

 - Turbo Cache CAS - The storage instance that stores all required build files
   and the build outputs.  This also acts as the AC (action cache) which stores
   the results of previous actions.

 - Turbo Cache Worker - There can be many of these and they perform the build
   work.  They are rate limited to perform one build per core using the
   cpu_count property.  The worker_precondition_script.sh ensures that they have
   sufficient RAM to perform a build before it starts.

## Running

To start Turbo-Cache simply execute `docker-compose up` from this directory.
A new Turbo-Cache instance with all of the backing will be built and started on
port 50052.

Then in a Chromium directory execute the following:

```sh
# Required because the default is projects/rbe-chrome-untrusted/instances/default_instance
# and we do not support instance names with / in currently.
export RBE_instance=main
export RBE_service=127.0.0.1:50052
# Implied by service_no_security=true, but here for reference
export RBE_service_no_auth=true
# Disables both TLS and authentication
export RBE_service_no_security=true
# Turbo-cache doesn't currently support compressed blob upload
export RBE_compression_threshold=-1
# Try not to use local execution, can't set to 0 otherwise it appears to ignore
export RBE_local_resource_fraction=0.0001
```

Now that the environment is set up you simply need to enable it for your
Chromium build by modifying your `args.gn` file to contain:

```
use_remoteexec=true
rbe_cfg_dir="../../buildtools/reclient_cfgs/linux"
```

Now run your build using autoninja which will automatically start reproxy and
do all of your execution for you.

You should set a crontab to clean up stale docker images nightly on workers:
```sh
docker image prune -a --filter "until=24h" -f
```
