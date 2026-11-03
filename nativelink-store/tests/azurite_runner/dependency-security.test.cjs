const assert = require("node:assert/strict");
const { createRequire } = require("node:module");
const { test } = require("node:test");

for (const consumer of ["@azure/ms-rest-js", "sequelize"]) {
	test(`${consumer} can use the patched CommonJS UUID dependency`, () => {
		const consumerRequire = createRequire(
			require.resolve(`${consumer}/package.json`),
		);
		const uuid = consumerRequire("uuid");
		assert.equal(uuid.version(uuid.v1()), 1);
		assert.equal(uuid.version(uuid.v4()), 4);

		// GHSA-w5hq-g745-h8pq: reject a short buffer without partially writing it.
		for (const generate of [uuid.v3, uuid.v5]) {
			const buffer = new Uint8Array(8).fill(0xaa);
			assert.throws(() => generate("test", uuid.v5.DNS, buffer, 4), RangeError);
			assert.deepEqual(buffer, new Uint8Array(8).fill(0xaa));
		}
	});
}

test("the Azure SDK and Sequelize still generate UUIDs through their public APIs", () => {
	const sdk = require("@azure/ms-rest-js");
	const { DataTypes, Utils } = require("sequelize");
	const uuid = require("uuid");
	assert.equal(uuid.version(sdk.generateUuid()), 4);
	assert.equal(uuid.version(Utils.toDefaultValue(new DataTypes.UUIDV1())), 1);
	assert.equal(uuid.version(Utils.toDefaultValue(new DataTypes.UUIDV4())), 4);
});

test("the telemetry SDK bounds incoming baggage entries", () => {
	const telemetryRequire = createRequire(
		require.resolve("applicationinsights/package.json"),
	);
	const { W3CBaggagePropagator } = telemetryRequire("@opentelemetry/core");
	const { ROOT_CONTEXT, defaultTextMapGetter, propagation } =
		telemetryRequire("@opentelemetry/api");
	const baggage = Array.from({ length: 200 }, (_, i) => `k${i}=v`).join(",");
	const context = new W3CBaggagePropagator().extract(
		ROOT_CONTEXT,
		{ baggage },
		defaultTextMapGetter,
	);
	assert.equal(propagation.getBaggage(context).getAllEntries().length, 180);
});

// Redirects a request across hosts (127.0.0.1 to localhost) and returns the
// headers the second host received.
async function headersAfterCrossHostRedirect(requestOptions) {
	const http = require("node:http");
	const { once } = require("node:events");
	const axiosRequire = createRequire(require.resolve("axios/package.json"));
	const followRedirects = axiosRequire("follow-redirects");

	let received;
	const target = http.createServer((req, res) => {
		received = req.headers;
		res.end();
	});
	target.listen(0);
	await once(target, "listening");
	const origin = http.createServer((_req, res) => {
		res.writeHead(302, {
			location: `http://localhost:${target.address().port}/`,
		});
		res.end();
	});
	origin.listen(0, "127.0.0.1");
	await once(origin, "listening");
	try {
		const req = followRedirects.http.request(
			`http://127.0.0.1:${origin.address().port}/`,
			requestOptions,
		);
		req.end();
		const [res] = await once(req, "response");
		res.resume();
		await once(res, "end");
	} finally {
		origin.close();
		target.close();
	}
	return received;
}

test("follow-redirects strips array-form credential headers across hosts", async () => {
	// GHSA-q3jh-wrc6-jgq8: array-form headers skipped the credential filter.
	const headers = await headersAfterCrossHostRedirect({
		headers: ["Authorization", "Bearer secret", "X-Trace", "kept"],
	});
	assert.equal(headers.authorization, undefined);
	assert.equal(headers["x-trace"], "kept");
});

test("follow-redirects strips declared sensitive headers containing regex metacharacters", async () => {
	// GHSA-8r9p-f939-6h3c: "|" and "^" in sensitiveHeaders were not escaped.
	const headers = await headersAfterCrossHostRedirect({
		headers: { "X-Api|Key": "secret", "X-Trace": "kept" },
		sensitiveHeaders: ["X-Api|Key"],
	});
	assert.equal(headers["x-api|key"], undefined);
	assert.equal(headers["x-trace"], "kept");
});

// GHSA-27r3-jxmh-p828: a compressed body followed by trailing bytes was read
// and answered normally, and the trailing bytes then reached the decompression
// stream body-parser had left piped with no error listener, which took the
// process down. The fix unpipes and destroys that stream once the body is read,
// and drains the rest of the request. Azurite reaches body-parser through
// Express, so the patched version arrives through the override in package.json.
const azuriteRequire = createRequire(require.resolve("azurite/package.json"));

test("Express resolves a body-parser that survives trailing compressed bytes", () => {
	const { version } = azuriteRequire("body-parser/package.json");
	const [major, minor] = version.split(".").map(Number);
	assert.ok(
		major > 2 || (major === 2 && minor >= 4),
		`body-parser ${version} predates the GHSA-27r3-jxmh-p828 fix in 2.4.0`,
	);
});

test("a request whose trailing bytes land after the body is read still finishes", async () => {
	const http = require("node:http");
	const zlib = require("node:zlib");
	const { once } = require("node:events");
	const parse = azuriteRequire("body-parser").json();

	// One complete frame, then bytes the decompressor only sees once it has
	// finished: the server sends them from inside the parser's callback.
	const frame = Buffer.concat([
		zlib.deflateSync(Buffer.from('{"blob":"nativelink"}')),
		Buffer.from([0x00]),
	]);
	const trailing = Buffer.alloc(1024);
	let client;

	const escaped = [];
	const collect = (error) => escaped.push(error);
	process.on("uncaughtException", collect);

	const server = http.createServer((req, res) => {
		parse(req, res, (err) => {
			if (err) {
				res.writeHead(400).end();
				return;
			}
			// Answer only once the whole request has finished, so a decompression
			// stream still holding the connection open fails the test.
			req.on("end", () => res.writeHead(200).end(JSON.stringify(req.body)));
			client.end(trailing);
		});
	});
	server.listen(0, "127.0.0.1");
	await once(server, "listening");

	try {
		const { status, body } = await new Promise((resolve, reject) => {
			client = http.request(
				{
					host: "127.0.0.1",
					port: server.address().port,
					method: "POST",
					agent: false,
					headers: {
						"content-type": "application/json",
						"content-encoding": "deflate",
						"content-length": frame.length + trailing.length,
					},
				},
				(res) => {
					let body = "";
					res.setEncoding("utf8");
					res.on("data", (chunk) => {
						body += chunk;
					});
					res.on("error", reject);
					res.on("end", () => resolve({ status: res.statusCode, body }));
				},
			);
			client.on("error", reject);
			client.setTimeout(5000, () =>
				client.destroy(new Error("the HTTP request never finished")),
			);
			client.write(frame);
		});

		assert.equal(status, 200);
		assert.deepEqual(JSON.parse(body), { blob: "nativelink" });
		assert.deepEqual(escaped, []);
	} finally {
		process.off("uncaughtException", collect);
		if (client) client.destroy();
		await new Promise((resolve) => server.close(resolve));
	}
});

// GHSA-xcx6-jx8p-x3h5: a confidential client trusted instance-aware cloud
// host names from the authorization code payload, so a crafted payload could
// redirect the token request. Azurite reaches MSAL Node through
// @azure/identity, so the patched version arrives through the override in
// package.json. The fix also rejects instance-aware confidential clients.
test("@azure/identity resolves an MSAL Node that rejects instance-aware confidential clients", () => {
	const identityRequire = createRequire(
		require.resolve("@azure/identity/package.json"),
	);
	const { ConfidentialClientApplication } = identityRequire("@azure/msal-node");
	assert.throws(
		() =>
			new ConfidentialClientApplication({
				auth: {
					clientId: "00000000-0000-0000-0000-000000000000",
					clientSecret: "secret",
					instanceAware: true,
				},
			}),
		{ errorCode: "unsupported_cca_instance_aware" },
	);
});
