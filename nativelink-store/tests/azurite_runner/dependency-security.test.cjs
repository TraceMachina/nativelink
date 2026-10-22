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

// Azurite reaches follow-redirects through axios. A cross-origin redirect
// must not carry credentials, whether the headers are an array (GHSA-q3jh-wrc6-jgq8)
// or a declared sensitive name contains a regex metacharacter (GHSA-8r9p-f939-6h3c).
test("follow-redirects strips credentials on a cross-origin redirect", async () => {
	const http = require("node:http");
	const axiosRequire = createRequire(require.resolve("axios/package.json"));
	const { http: followHttp } = axiosRequire("follow-redirects");

	const listen = (handler) =>
		new Promise((resolve) => {
			const server = http.createServer(handler);
			server.listen(0, "127.0.0.1", () => resolve(server));
		});
	const received = [];
	const target = await listen((req, res) => {
		received.push(req.headers);
		res.end("ok");
	});
	const origin = await listen((_req, res) => {
		res.writeHead(302, {
			location: `http://127.0.0.1:${target.address().port}/`,
		});
		res.end();
	});
	const fetchThroughRedirect = (options) =>
		new Promise((resolve) => {
			const req = followHttp.request(
				{ host: "127.0.0.1", port: origin.address().port, path: "/", ...options },
				(res) => res.resume().on("end", () => resolve(null)),
			);
			req.on("error", resolve);
			req.end();
		});

	try {
		// A loud rejection of array headers is as safe as stripping them.
		const arrayError = await fetchThroughRedirect({
			headers: ["Authorization", "Bearer secret", "Cookie", "session=secret"],
		});
		const declaredError = await fetchThroughRedirect({
			headers: { "X-Api^Key": "secret", "X-Token|Secret": "secret" },
			sensitiveHeaders: ["X-Api^Key", "X-Token|Secret"],
		});
		assert.equal(declaredError, null);
		assert.equal(received.length, arrayError ? 1 : 2);
		for (const headers of received) {
			for (const name of ["authorization", "cookie", "x-api^key", "x-token|secret"]) {
				assert.equal(headers[name], undefined, `${name} leaked across origins`);
			}
		}
	} finally {
		origin.close();
		target.close();
	}
});
