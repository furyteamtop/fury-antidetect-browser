// Test stand-in for the desktop `api` module. Tests assign the methods they
// drive; reaching anything else is loud, so an unstubbed path cannot pass
// silently.
export const api = new Proxy({}, {
  get(target, name) {
    if (name in target) return target[name];
    throw new Error(`api.${String(name)} is not stubbed by this test`);
  },
});
