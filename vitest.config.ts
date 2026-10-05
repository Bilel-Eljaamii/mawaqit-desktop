import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "happy-dom",
    include: ["tests/frontend/**/*.test.ts"],
    coverage: {
      provider: "v8",
      reporter: ["text", "html"],
      reportsDirectory: "coverage/frontend",
      include: ["src/lib/**"],
      thresholds: {
        lines: 90,
        functions: 95,
        branches: 90,
        statements: 90,
      },
    },
  },
});
