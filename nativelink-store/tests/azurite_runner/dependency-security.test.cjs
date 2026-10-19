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
