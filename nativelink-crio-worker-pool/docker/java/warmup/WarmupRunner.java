/**
 * JVM Warmup Runner
 *
 * This class exercises the JVM's JIT compiler and class loading subsystem
 * to bring the runtime to a "hot" state before serving real build requests.
 */
public class WarmupRunner {

    public static void main(String[] args) {
        int iterations = 100;
        if (args.length > 0) {
            iterations = Integer.parseInt(args[0]);
        }

        System.out.println("Starting JVM warmup with " + iterations + " iterations");
        long startTime = System.currentTimeMillis();

        // Exercise hot code paths for JIT compilation
        for (int i = 0; i < iterations; i++) {
            if (i % 10 == 0) {
                System.out.println("Warmup iteration " + i + "/" + iterations);
            }

            // Computational work to trigger JIT
            performComputations();

            // String operations (common in build tools)
            performStringOperations();

            // Collection operations
            performCollectionOperations();

            // I/O simulation
            performIoOperations();
        }

        long duration = System.currentTimeMillis() - startTime;
        System.out.println("Warmup complete in " + duration + "ms");
    }

    private static void performComputations() {
        double result = 0.0;
        for (int i = 0; i < 1000; i++) {
            result += Math.sqrt(i) * Math.random();
            result = Math.sin(result) + Math.cos(result);
        }
        // Prevent dead code elimination
        if (result > 1e10) {
            System.out.println(result);
        }
    }

    private static void performStringOperations() {
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < 100; i++) {
            sb.append("iteration_").append(i).append("_");
            String s = sb.toString();
            s = s.toUpperCase();
            s = s.toLowerCase();
            s = s.replace("_", "-");
        }
    }

    private static void performCollectionOperations() {
        java.util.List<String> list = new java.util.ArrayList<>();
        for (int i = 0; i < 100; i++) {
            list.add("item_" + i);
        }

        java.util.Map<String, Integer> map = new java.util.HashMap<>();
        for (int i = 0; i < 100; i++) {
            map.put("key_" + i, i);
        }

        // Iteration
        list.stream()
            .filter(s -> s.contains("5"))
            .map(String::toUpperCase)
            .forEach(s -> {});
    }

    private static void performIoOperations() {
        try {
            // File operations that are common in builds
            java.io.File tmpFile = java.io.File.createTempFile("warmup", ".tmp");
            try (java.io.PrintWriter writer = new java.io.PrintWriter(tmpFile)) {
                writer.println("warmup data");
            }
            tmpFile.delete();
        } catch (java.io.IOException e) {
            // Ignore warmup errors
        }
    }
}
