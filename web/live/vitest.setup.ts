import { beforeEach } from "vitest";
import { cleanup } from "@solidjs/testing-library";
import './src/index.css'
import 'material-symbols';
// Before each test, we run cleanup to remove any mounted components from previous tests
beforeEach(() => {
  cleanup();
})
