// Registers the test resolution shim. See proxyline-loader.mjs.
import { register } from "node:module";

register(new URL("./proxyline-loader.mjs", import.meta.url));
