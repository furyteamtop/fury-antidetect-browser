// Test-only resolution shim for running the desktop sources under the plain
// Node runner without npm dependencies:
//
//   * `./api` imported from a `.ts` source maps to `api-stub.mjs`, because the
//     real `api.ts` uses TypeScript parameter properties, which Node's
//     strip-only TypeScript mode cannot run, and because the tests stub the
//     api methods themselves. The tests import the same stub to patch it.
//   * Extensionless relative imports inside `.ts` files (`./api`) get `.ts`
//     appended, which Node's TypeScript stripping does not do on its own.
//
// Nothing here is part of the app; `node --test` never runs it in production.

const apiStub = new URL("./api-stub.mjs", import.meta.url).href;

export async function resolve(specifier, context, next) {
  const parent = context.parentURL ?? "";
  if (parent.endsWith(".ts") && specifier === "./api") {
    return { url: apiStub, shortCircuit: true };
  }
  if (
    parent.endsWith(".ts") &&
    (specifier.startsWith("./") || specifier.startsWith("../")) &&
    !/\.[^./]+$/.test(specifier)
  ) {
    return next(`${specifier}.ts`, context);
  }
  return next(specifier, context);
}
