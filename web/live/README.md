# nativelink-live
Live feed viewer for NativeLink.
This is useful when you want to see what's happening under the hood.

## State
This is alpha software. It's not recommended for production use and not covered by SLA.

## Usage
<!-- maybe live.nativelink.com -->
You can use the website we host(TBA), or you can self-host it yourself.
Since this is client-side only static web, you only need to get the service that serves these static files.

To build the static files, you need to have Node.js and `npm` installed.
Then, run the following commands:

```bash
npm install
npm run build
```

This will create a `dist` directory with the static files.
You can serve this directory with any static file server, such as `nginx`, `caddy`, or `serve`.

TODO(ilsubyeega): add example configuration for serving this assets.

While for development, you may want to use `npm run dev` to start a local development server with hot-reload instead of building.

## Screenshot
TBA

## Goals
- Minimal code; minimal implementation.

## Non-goals
- Invoking something on the NativeLink side; this is a read-only viewer.
- dedicated tenant support and auth; should be handled by reverse proxy in front of the app
- support old browser, this uses cutting-edge web technologies.

## License

Copyright 2020–2025 Trace Machina, Inc.

Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License.
SPDX identifier: `FSL-1.1-Apache-2.0`.

After the second anniversary of the date this version was made available, you may use this
software under the Apache License, Version 2.0.
