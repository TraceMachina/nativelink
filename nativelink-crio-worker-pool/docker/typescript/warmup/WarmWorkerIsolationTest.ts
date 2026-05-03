/**
 * TypeScript port of WarmWorkerIsolationTest.java.
 *
 * Demonstrates the warm-worker isolation difference introduced by the
 * Copy-on-Write (COW) overlay model in `nativelink-crio-worker-pool`.
 *
 * Without isolation a long-lived Node.js worker reuses the same writable
 * filesystem across tenants, so secrets, build artifacts, and `node_modules`
 * mutations leak from one job into the next. With per-job COW isolation each
 * job lands on its own upper layer stacked on the warmed template (lower);
 * reads fall through to the template, writes are scoped to the upper, and
 * cleanup discards the upper while the template - and its primed module
 * cache - survives.
 *
 * This is a pure-Node model of `OverlayFsMount` in
 * `nativelink-crio-worker-pool/src/isolation.rs`. It does not require root,
 * OverlayFS, or CRI-O - the isolation logic itself is what we verify.
 *
 * Run:
 *   tsc WarmWorkerIsolationTest.ts && node WarmWorkerIsolationTest.js
 * Exits non-zero on assertion failure.
 */
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

const TEMPLATE_FILE = "v8-bytecode-cache.dat";
const TEMPLATE_BODY = "warmed-turbofan-profile";
const SECRET_FILE = "tenant-secret.txt";
const TENANT_A_SECRET = "AKIA-tenant-a-private-key";

let passed = 0;
let failed = 0;

function main(): number {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "warm-worker-isolation-"));
    try {
        console.log("=== Warm Worker Isolation: before vs after ===");
        console.log(`workspace: ${root}`);
        console.log("");

        runUnsafeWarmWorkerScenario(path.join(root, "unsafe"));
        console.log("");
        runCowIsolatedScenario(path.join(root, "safe"));

        console.log("");
        console.log(`passed=${passed} failed=${failed}`);
        return failed > 0 ? 1 : 0;
    } finally {
        fs.rmSync(root, { recursive: true, force: true });
    }
}

/** "Before" - one warm worker reused across tenants. Tenant A's writes survive. */
function runUnsafeWarmWorkerScenario(scenarioRoot: string): void {
    console.log("[before] shared warm worker (no isolation)");

    const sharedWorkspace = path.join(scenarioRoot, "worker");
    fs.mkdirSync(sharedWorkspace, { recursive: true });
    // Simulate the warmup phase having populated some on-disk state.
    fs.writeFileSync(path.join(sharedWorkspace, TEMPLATE_FILE), TEMPLATE_BODY);

    // Tenant A runs, drops a secret on the worker, then "finishes".
    fs.writeFileSync(path.join(sharedWorkspace, SECRET_FILE), TENANT_A_SECRET);

    // Tenant B is scheduled onto the SAME warm worker.
    const templateStillThere =
        readOrEmpty(path.join(sharedWorkspace, TEMPLATE_FILE)) === TEMPLATE_BODY;
    const secretLeaked =
        readOrEmpty(path.join(sharedWorkspace, SECRET_FILE)) === TENANT_A_SECRET;

    assertTrue("warmup state survives across jobs (perf preserved)", templateStillThere);
    assertTrue(
        "tenant A's secret leaks into tenant B's job (the bug this PR fixes)",
        secretLeaked,
    );
}

/** "After" - per-job COW overlays. Reads fall through; writes are scoped. */
function runCowIsolatedScenario(scenarioRoot: string): void {
    console.log("[after] COW-isolated warm worker");

    const template = path.join(scenarioRoot, "template");
    const jobs = path.join(scenarioRoot, "jobs");
    fs.mkdirSync(template, { recursive: true });
    // Warmup populates the template once.
    fs.writeFileSync(path.join(template, TEMPLATE_FILE), TEMPLATE_BODY);

    // Tenant A acquires an isolated clone, writes a secret, releases.
    const tenantA = OverlayWorkspace.create(template, jobs, "job-tenant-a");
    tenantA.write(SECRET_FILE, TENANT_A_SECRET);
    assertEqual(
        "tenant A sees its own write inside its overlay",
        TENANT_A_SECRET,
        tenantA.read(SECRET_FILE),
    );
    assertEqual(
        "tenant A reads template through the lower layer",
        TEMPLATE_BODY,
        tenantA.read(TEMPLATE_FILE),
    );
    tenantA.cleanup();

    // Tenant B acquires a fresh isolated clone of the same template.
    const tenantB = OverlayWorkspace.create(template, jobs, "job-tenant-b");
    try {
        assertEqual(
            "tenant B still benefits from warmup (template file present)",
            TEMPLATE_BODY,
            tenantB.read(TEMPLATE_FILE),
        );
        assertEqual(
            "tenant A's secret is NOT visible to tenant B",
            "",
            tenantB.read(SECRET_FILE),
        );
        assertTrue(
            "template lower layer is untouched",
            fs.existsSync(path.join(template, TEMPLATE_FILE)),
        );
    } finally {
        tenantB.cleanup();
    }
}

/**
 * Pure-Node mirror of `OverlayFsMount` from `isolation.rs`: reads probe
 * upper first, then fall through to lower; writes always land in upper;
 * cleanup deletes upper/work/merged but leaves lower intact.
 */
class OverlayWorkspace {
    private constructor(
        private readonly lower: string,
        private readonly upper: string,
        private readonly work: string,
        private readonly merged: string,
    ) {}

    static create(lower: string, jobsRoot: string, jobId: string): OverlayWorkspace {
        const jobDir = path.join(jobsRoot, jobId);
        const upper = path.join(jobDir, "upper");
        const work = path.join(jobDir, "work");
        const merged = path.join(jobDir, "merged");
        fs.mkdirSync(upper, { recursive: true });
        fs.mkdirSync(work, { recursive: true });
        fs.mkdirSync(merged, { recursive: true });
        return new OverlayWorkspace(lower, upper, work, merged);
    }

    write(relativePath: string, contents: string): void {
        const target = path.join(this.upper, relativePath);
        fs.mkdirSync(path.dirname(target), { recursive: true });
        fs.writeFileSync(target, contents);
    }

    read(relativePath: string): string {
        const upperPath = path.join(this.upper, relativePath);
        if (fs.existsSync(upperPath)) {
            return fs.readFileSync(upperPath, "utf8");
        }
        const lowerPath = path.join(this.lower, relativePath);
        if (fs.existsSync(lowerPath)) {
            return fs.readFileSync(lowerPath, "utf8");
        }
        return "";
    }

    cleanup(): void {
        for (const dir of [this.upper, this.work, this.merged]) {
            fs.rmSync(dir, { recursive: true, force: true });
        }
        const parent = path.dirname(this.upper);
        if (fs.existsSync(parent)) {
            fs.rmSync(parent, { recursive: true, force: true });
        }
    }
}

function readOrEmpty(p: string): string {
    return fs.existsSync(p) ? fs.readFileSync(p, "utf8") : "";
}

function assertTrue(description: string, condition: boolean): void {
    record(condition, description);
}

function assertEqual(description: string, expected: string, actual: string): void {
    const ok = expected === actual;
    record(
        ok,
        `${description} (expected="${expected}", actual="${actual}")`,
    );
}

function record(ok: boolean, description: string): void {
    if (ok) {
        passed++;
        console.log(`  PASS  ${description}`);
    } else {
        failed++;
        console.log(`  FAIL  ${description}`);
    }
}

process.exit(main());
