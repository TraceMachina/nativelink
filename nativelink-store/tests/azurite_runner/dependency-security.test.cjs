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
