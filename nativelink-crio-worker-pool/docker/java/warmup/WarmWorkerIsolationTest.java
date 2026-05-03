import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Comparator;
import java.util.stream.Stream;

/**
 * Demonstrates the warm-worker isolation difference introduced by the
 * Copy-on-Write (COW) overlay model in {@code nativelink-crio-worker-pool}.
 *
 * <p>The CRI-O warm-worker pool keeps a long-lived "template" container that has
 * already paid the JVM warmup cost. Without isolation, every job that lands on
 * the warm worker shares the same writable filesystem as every previous job, so
 * a secret written by tenant A is visible to tenant B (the leak this PR fixes).
 *
 * <p>With COW isolation each job mounts its own upper/work directories on top
 * of the shared lower (template) layer. Reads fall through to the template;
 * writes are confined to the per-job upper layer; cleanup discards the upper
 * layer. The template - and its warmup state - survives across jobs.
 *
 * <p>This is a pure-Java model of {@code OverlayFsMount} in
 * {@code nativelink-crio-worker-pool/src/isolation.rs}. It does not require
 * root, OverlayFS, or CRI-O - the isolation logic itself is what we verify.
 *
 * <p>Run standalone:
 * <pre>
 *   javac WarmWorkerIsolationTest.java
 *   java WarmWorkerIsolationTest
 * </pre>
 * Exits non-zero on assertion failure.
 */
public final class WarmWorkerIsolationTest {

    private static final String TEMPLATE_FILE = "jvm-class-cache.dat";
    private static final String TEMPLATE_BODY = "warmed-jit-profile";
    private static final String SECRET_FILE = "tenant-secret.txt";
    private static final String TENANT_A_SECRET = "AKIA-tenant-a-private-key";

    private static int passed;
    private static int failed;

    public static void main(String[] args) throws IOException {
        Path root = Files.createTempDirectory("warm-worker-isolation-");
        try {
            System.out.println("=== Warm Worker Isolation: before vs after ===");
            System.out.println("workspace: " + root);
            System.out.println();

            runUnsafeWarmWorkerScenario(root.resolve("unsafe"));
            System.out.println();
            runCowIsolatedScenario(root.resolve("safe"));

            System.out.println();
            System.out.println("passed=" + passed + " failed=" + failed);
            if (failed > 0) {
                System.exit(1);
            }
        } finally {
            deleteRecursively(root);
        }
    }

    /**
     * "Before" - one warm worker reused across tenants. Whatever tenant A wrote
     * is still on disk when tenant B lands on the same worker.
     */
    private static void runUnsafeWarmWorkerScenario(Path scenarioRoot) throws IOException {
        System.out.println("[before] shared warm worker (no isolation)");

        Path sharedWorkspace = scenarioRoot.resolve("worker");
        Files.createDirectories(sharedWorkspace);
        // Simulate the warmup phase having populated some on-disk state.
        Files.writeString(sharedWorkspace.resolve(TEMPLATE_FILE), TEMPLATE_BODY);

        // Tenant A runs, drops a secret on the worker, then "finishes".
        Files.writeString(sharedWorkspace.resolve(SECRET_FILE), TENANT_A_SECRET);

        // Tenant B is scheduled onto the SAME warm worker.
        boolean templateStillThere =
                TEMPLATE_BODY.equals(readOrEmpty(sharedWorkspace.resolve(TEMPLATE_FILE)));
        boolean secretLeaked =
                TENANT_A_SECRET.equals(readOrEmpty(sharedWorkspace.resolve(SECRET_FILE)));

        assertTrue("warmup state survives across jobs (perf preserved)", templateStillThere);
        assertTrue(
                "tenant A's secret leaks into tenant B's job (the bug this PR fixes)",
                secretLeaked);
    }

    /**
     * "After" - each job gets a COW overlay. Reads fall through to the shared
     * template, writes go to a per-job upper layer that is discarded on
     * cleanup. Tenant B never sees what tenant A wrote.
     */
    private static void runCowIsolatedScenario(Path scenarioRoot) throws IOException {
        System.out.println("[after] COW-isolated warm worker");

        Path template = scenarioRoot.resolve("template");
        Path jobs = scenarioRoot.resolve("jobs");
        Files.createDirectories(template);
        // Warmup populates the template once.
        Files.writeString(template.resolve(TEMPLATE_FILE), TEMPLATE_BODY);

        // Tenant A acquires an isolated clone, writes a secret, releases.
        OverlayWorkspace tenantA = OverlayWorkspace.create(template, jobs, "job-tenant-a");
        tenantA.write(SECRET_FILE, TENANT_A_SECRET);
        assertEquals(
                "tenant A sees its own write inside its overlay",
                TENANT_A_SECRET,
                tenantA.read(SECRET_FILE));
        assertEquals(
                "tenant A reads template through the lower layer",
                TEMPLATE_BODY,
                tenantA.read(TEMPLATE_FILE));
        tenantA.cleanup();

        // Tenant B acquires a fresh isolated clone of the same template.
        OverlayWorkspace tenantB = OverlayWorkspace.create(template, jobs, "job-tenant-b");
        try {
            assertEquals(
                    "tenant B still benefits from warmup (template file present)",
                    TEMPLATE_BODY,
                    tenantB.read(TEMPLATE_FILE));
            assertEquals(
                    "tenant A's secret is NOT visible to tenant B",
                    "",
                    tenantB.read(SECRET_FILE));
            assertTrue("template lower layer is untouched", Files.exists(template.resolve(TEMPLATE_FILE)));
        } finally {
            tenantB.cleanup();
        }
    }

    /**
     * Pure-Java mirror of {@code OverlayFsMount} from {@code isolation.rs}:
     * reads probe upper first, then fall through to lower; writes always land
     * in upper; cleanup deletes upper/work/merged but leaves lower intact.
     */
    private static final class OverlayWorkspace {
        private final Path lower;
        private final Path upper;
        private final Path work;
        private final Path merged;

        private OverlayWorkspace(Path lower, Path upper, Path work, Path merged) {
            this.lower = lower;
            this.upper = upper;
            this.work = work;
            this.merged = merged;
        }

        static OverlayWorkspace create(Path lower, Path jobsRoot, String jobId) throws IOException {
            Path jobDir = jobsRoot.resolve(jobId);
            Path upper = jobDir.resolve("upper");
            Path work = jobDir.resolve("work");
            Path merged = jobDir.resolve("merged");
            Files.createDirectories(upper);
            Files.createDirectories(work);
            Files.createDirectories(merged);
            return new OverlayWorkspace(lower, upper, work, merged);
        }

        void write(String relativePath, String contents) throws IOException {
            Path target = upper.resolve(relativePath);
            Files.createDirectories(target.getParent() == null ? upper : target.getParent());
            Files.writeString(target, contents);
        }

        String read(String relativePath) throws IOException {
            Path upperPath = upper.resolve(relativePath);
            if (Files.exists(upperPath)) {
                return Files.readString(upperPath, StandardCharsets.UTF_8);
            }
            Path lowerPath = lower.resolve(relativePath);
            if (Files.exists(lowerPath)) {
                return Files.readString(lowerPath, StandardCharsets.UTF_8);
            }
            return "";
        }

        void cleanup() throws IOException {
            deleteRecursively(upper);
            deleteRecursively(work);
            deleteRecursively(merged);
            // Remove the parent job directory if it's now empty.
            Path parent = upper.getParent();
            if (parent != null && Files.exists(parent)) {
                deleteRecursively(parent);
            }
        }
    }

    private static String readOrEmpty(Path p) throws IOException {
        return Files.exists(p) ? Files.readString(p, StandardCharsets.UTF_8) : "";
    }

    private static void deleteRecursively(Path root) throws IOException {
        if (!Files.exists(root)) {
            return;
        }
        try (Stream<Path> walk = Files.walk(root)) {
            walk.sorted(Comparator.reverseOrder()).forEach(p -> {
                try {
                    Files.deleteIfExists(p);
                } catch (IOException ignored) {
                    // best-effort cleanup
                }
            });
        }
    }

    private static void assertTrue(String description, boolean condition) {
        record(condition, description);
    }

    private static void assertEquals(String description, String expected, String actual) {
        boolean ok = expected.equals(actual);
        record(ok, description + " (expected=\"" + expected + "\", actual=\"" + actual + "\")");
    }

    private static void record(boolean ok, String description) {
        if (ok) {
            passed++;
            System.out.println("  PASS  " + description);
        } else {
            failed++;
            System.out.println("  FAIL  " + description);
        }
    }
}
