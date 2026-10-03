// Regression test for the paste race in `spreadPastedProxy` (KALPAK-187).
//
// Runs under Node's built-in test runner; `proxyline-register.mjs` supplies
// the module resolution the sources need (see proxyline-loader.mjs):
//
//   npm test
//
// HELPER_PATH, when set, loads a different copy of the helper. It exists to
// prove this regression against the unfixed upstream helper; unset it always
// runs the real source.
import { test } from "node:test";
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";

/** A promise the test resolves by hand, so ordering is explicit and no
 *  timing is left to chance. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

/** Runs every microtask queued so far, deterministically: a macrotask
 *  boundary cannot start before the microtask queue is empty. */
const flush = () => new Promise((r) => setTimeout(r, 0));

async function loadHelper() {
  const helperUrl = process.env.HELPER_PATH
    ? pathToFileURL(process.env.HELPER_PATH).href
    : new URL("../src/proxyLine.ts", import.meta.url).href;
  // The loader maps the sources' own `./api` import to the same stub, so
  // patching it here drives the helper whatever copy of it runs.
  const { api } = await import(new URL("./api-stub.mjs", import.meta.url).href);
  const { spreadPastedProxy } = await import(helperUrl);
  return { api, spreadPastedProxy };
}

/** A paste event over a field. Related pastes pass the same field: the real
 *  DOM input is one node that every paste, edit and late answer reads and
 *  writes, not a fresh object per event. */
function pasteEvent(text: string, field = { value: "", selectionStart: 0, selectionEnd: 0 }) {
  const event = {
    clipboardData: { getData: () => text },
    preventDefault: () => {},
    currentTarget: field,
  } as unknown as ClipboardEvent<HTMLInputElement>;
  return { event, field };
}

/** The smallest stand-in for ProxyForm's host field: what a paste applies and
 *  what a sniff may still set, with the same guards the component has. The
 *  field, when given, is the DOM node the helper reads back, so the mimic
 *  commits an applied host into it the way React does after a state update. */
function formState(field?: { value: string }) {
  return {
    kind: "socks5",
    kindTouched: false,
    sniffed: null as string | null,
    host: "",
    port: "",
    user: "",
    pass: "",
    apply(p: { kind?: string; host: string; port: string; username?: string; password?: string }) {
      this.kindTouched = false;
      this.sniffed = null;
      this.host = p.host;
      if (field) field.value = p.host;
      if (p.port) this.port = p.port;
      if (p.kind) this.kind = p.kind;
      if (p.username !== undefined) this.user = p.username;
      if (p.password !== undefined) this.pass = p.password;
    },
    onKind(k: "http" | "socks5") {
      if (this.kindTouched) return;
      this.kind = k;
      this.sniffed = k;
    },
    editHost(v: string) {
      // Mirrors the host field's onChange, which marks the kind touched:
      // an edit after the paste outranks what the address said.
      this.kindTouched = true;
      this.host = v;
      if (field) field.value = v;
    },
  };
}

interface Parsed {
  kind: string;
  host: string;
  port: number;
  username: string | null;
  password: string | null;
  shape: "Url" | "HostPort" | "HostPortUserPass" | "AtSign";
}

test("a late answer for an older paste cannot change the form the newest paste filled", async () => {
  const { api, spreadPastedProxy } = await loadHelper();
  const s = formState();
  const handler = spreadPastedProxy((p) => s.apply(p), (k) => s.onKind(k));

  const oldParse = deferred<Parsed | null>();
  const oldSniff = deferred<{ kind: "http" | "socks5" | null }>();
  const urlParsed: Parsed = {
    kind: "socks5",
    host: "5.6.7.8",
    port: 2222,
    username: null,
    password: null,
    shape: "Url",
  };
  api.parseProxyLine = (line: string) =>
    line === "9.9.9.9:1111:u:p" ? oldParse.promise : Promise.resolve(urlParsed);
  api.sniffProxyKind = () => oldSniff.promise;

  // Both pastes go through the same handler and the same field: paste one,
  // a supplier's host:port:user:pass with its parser answer still on the
  // way, then paste two, an explicit socks5 URL whose answer lands at once.
  const first = pasteEvent("9.9.9.9:1111:u:p");
  handler(first.event);
  await flush();
  const second = pasteEvent("socks5://5.6.7.8:2222", first.field);
  handler(second.event);
  await flush();
  assert.equal(s.host, "5.6.7.8");
  assert.equal(s.port, "2222");
  assert.equal(s.kind, "socks5");

  // The operator is not holding still either: while the old answers travel,
  // the host field is edited by hand.
  s.editHost("5.6.7.8-amended");

  // The old parse lands late, and the kind sniff it started lands after it.
  oldParse.resolve({
    kind: "http",
    host: "9.9.9.9",
    port: 1111,
    username: "u",
    password: "p",
    shape: "HostPortUserPass",
  });
  await flush();
  oldSniff.resolve({ kind: "http" });
  await flush();

  // The newest paste must still own every field it set, and the operator's
  // edit must survive both late answers.
  assert.equal(s.host, "5.6.7.8-amended");
  assert.equal(s.port, "2222");
  assert.equal(s.kind, "socks5");
  assert.equal(s.user, "");
  assert.equal(s.pass, "");
});

test("a parser failure falls back to a plain paste and rejects nothing unhandled", async () => {
  const { api, spreadPastedProxy } = await loadHelper();
  const s = formState();
  const handler = spreadPastedProxy((p) => s.apply(p), () => {});
  api.parseProxyLine = () => Promise.reject(new Error("agent is down"));
  api.sniffProxyKind = () => Promise.resolve({ kind: null });

  handler(pasteEvent("host.example:1234").event);
  await flush();

  // The line the parser never read is pasted as the plain paste would have
  // put it, not dropped.
  assert.equal(s.host, "host.example:1234");
  assert.equal(s.port, "");
});

test("a handler rebuilt by the next render still guards an older paste's late answer", async () => {
  const { api, spreadPastedProxy } = await loadHelper();
  const s = formState();
  // ProxyForm builds a new handler on every render; render two must not
  // reset the paste ordering that render one's sniff compares against.
  const handler1 = spreadPastedProxy((p) => s.apply(p), (k) => s.onKind(k));
  const handler2 = spreadPastedProxy((p) => s.apply(p), (k) => s.onKind(k));

  const oldParse = deferred<Parsed | null>();
  const oldSniff = deferred<{ kind: "http" | "socks5" | null }>();
  const urlParsed: Parsed = {
    kind: "socks5",
    host: "5.6.7.8",
    port: 2222,
    username: null,
    password: null,
    shape: "Url",
  };
  api.parseProxyLine = (line: string) =>
    line === "9.9.9.9:1111:u:p" ? oldParse.promise : Promise.resolve(urlParsed);
  api.sniffProxyKind = () => oldSniff.promise;

  handler1(pasteEvent("9.9.9.9:1111:u:p").event);
  await flush();
  // Render two, then the newest paste through the fresh handler: an
  // explicit socks5 URL, whose answer lands at once.
  handler2(pasteEvent("socks5://5.6.7.8:2222").event);
  await flush();
  assert.equal(s.host, "5.6.7.8");
  assert.equal(s.kind, "socks5");

  // The old parse and the sniff it started land after the newer paste.
  oldParse.resolve({
    kind: "http",
    host: "9.9.9.9",
    port: 1111,
    username: "u",
    password: "p",
    shape: "HostPortUserPass",
  });
  await flush();
  oldSniff.resolve({ kind: "http" });
  await flush();

  assert.equal(s.host, "5.6.7.8");
  assert.equal(s.port, "2222");
  assert.equal(s.kind, "socks5");
});

test("a host edit while the parse is pending outranks the late answer", async () => {
  const { api, spreadPastedProxy } = await loadHelper();
  const field = { value: "", selectionStart: 0, selectionEnd: 0 };
  const s = formState(field);
  const handler = spreadPastedProxy((p) => s.apply(p), (k) => s.onKind(k));
  const parse = deferred<Parsed | null>();
  api.parseProxyLine = () => parse.promise;
  api.sniffProxyKind = () => Promise.resolve({ kind: null });

  handler(pasteEvent("9.9.9.9:1111:u:p", field).event);
  await flush();
  // The operator does not wait for the parse: the field is edited by hand,
  // and the field the helper read at paste time shows the edit.
  s.editHost("edited.example");

  parse.resolve({
    kind: "http",
    host: "9.9.9.9",
    port: 1111,
    username: "u",
    password: "p",
    shape: "HostPortUserPass",
  });
  await flush();

  // The late answer describes a line that is no longer in the field: it
  // must not overwrite the edit, and must not start its sniff either.
  assert.equal(s.host, "edited.example");
  assert.equal(s.user, "");
  assert.equal(s.pass, "");
});
