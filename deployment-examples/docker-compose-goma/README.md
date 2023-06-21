# Goma deployment

This example shows how to set up a Turbo-Cache with a Goma front end.  This
configuration is an example that runs all of the services on a single machine
which is obviously pointless, however it is a good starting point to allow you
to move the CAS and Worker roles to other machines.

When using Goma, all Chromium object files will be built on remote workers.
It will still compile certain things locally, such as Mojo, embedded.c, and all
linking operations.

## Roles

There a number of roles required to perform a Goma set up:

 - Goma remote exec proxy - This is the proxy between the Goma client and an RBE
   implementation (in this case, Turbo Cache).  The implementation is part of
   the Chromium source, but requires a few tweaks to function correctly with
   Turbo Cache.  This implementation also disables authentication.

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

To start Goma simply execute `docker-compose up` from this directory.  A new
Goma instance with all of the backing will be built and started on port 5051.

You will need to install the Goma client from your Chromium depot_tools:

```sh
./depot_tools/cipd install infra/goma/client/linux-amd64 -root ~/goma
```

Then in a Chromium directory execute the following:

```sh
export GOMA_SERVER_HOST="127.0.0.1"
export GOMA_SERVER_PORT="5051"
export GOMA_USE_SSL="false"
export GOMA_ARBITRARY_TOOLCHAIN_SUPPORT=true
export GOMACTL_USE_PROXY=false
export GOMA_USE_LOCAL=false
export GOMA_FALLBACK=true
export GOMA_START_COMPILER_PROXY=true
python3 ~/goma/goma_ctl.py ensure_start
```

Now that the Goma client is set up you simply need to enable it for your
Chromium build by modifying your `args.gn` file to contain:

```
use_goma = true
goma_dir = "/home/me/goma"
```

You will now want to increase your `-j` for `ninja` to around 10x your local
CPU core count.  Increasing past this is generally not recommended.

## Speed

If you get this all set up and distributed correctly, you should be able to
pull a clean build off the storage cache within around 13 minutes.  The time to
build object files will obviously depend on the size and power of your worker
nodes.
