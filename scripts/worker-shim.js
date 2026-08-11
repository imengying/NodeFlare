import * as imports from "./index_bg.js";
export * from "./index_bg.js";
import wasmModule from "./index.wasm";
import { WorkerEntrypoint } from "cloudflare:workers";
$SNIPPET_JS_IMPORTS

const instance = new WebAssembly.Instance(wasmModule, {
  "./index_bg.js": imports,
  $SNIPPET_WASM_IMPORTS
});

imports.__wbg_set_wasm(instance.exports);
instance.exports.__wbindgen_start?.();

export { wasmModule };

class Entrypoint extends WorkerEntrypoint {
  async fetch(request) {
    let response = imports.fetch(request, this.env, this.ctx);
    $WAIT_UNTIL_RESPONSE;
    return await response;
  }

  async scheduled(event) {
    return await imports.scheduled(event, this.env, this.ctx);
  }
}

const EXCLUDE_EXPORT = [
  "IntoUnderlyingByteSource",
  "IntoUnderlyingSink",
  "IntoUnderlyingSource",
  "MinifyConfig",
  "PolishConfig",
  "R2Range",
  "RequestRedirect",
  "fetch",
  "scheduled",
  "getMemory",
];

Object.keys(imports).map((key) => {
  if (!(EXCLUDE_EXPORT.includes(key) | key.startsWith("__"))) {
    Entrypoint.prototype[key] = imports[key];
  }
});

export default Entrypoint;
