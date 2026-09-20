import { GlobalRegistrator } from "@happy-dom/global-registrator";
GlobalRegistrator.register();

// @testing-library/react only auto-registers its afterEach(cleanup) when a
// global `afterEach` exists at import time; Bun's test runner does not put
// one on globalThis (it must be imported from "bun:test"), so without this
// renders from one test leak into the next within the same file. Register
// cleanup explicitly so tests are isolated.
//
// This must be a dynamic import: a static `import` here would be hoisted
// above the `GlobalRegistrator.register()` call above, so
// `@testing-library/dom`'s `screen` (bound once, at module-evaluation time,
// to whatever `document` exists then) would permanently capture an
// undefined `document` and every `screen.getByRole(...)` call would throw
// "a global document has to be available".
const { afterEach } = await import("bun:test");
const { cleanup } = await import("@testing-library/react");
afterEach(() => {
  cleanup();
});
