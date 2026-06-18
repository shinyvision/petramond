// Worker host: runs inside a Web Worker, loads the worker_wasm JS module,
// wires onmessage -> worker_entry -> postMessage with ArrayBuffer transfer.
//
// We defer installing the message listener until after wasm init so we don't
// process requests before `worker_entry` is available.

import init, { worker_entry } from "./worker_wasm.js";

// Await wasm init before handling messages. Requests that arrive during
// init are queued and flushed once we're ready.
const queue = [];
let ready = false;

self.onmessage = (ev) => {
  const data = ev.data;
  // Only handle binary payloads (Uint8Array / ArrayBuffer). Other messages
  // (control pings) are ignored.
  if (!(data instanceof Uint8Array) && !(data instanceof ArrayBuffer)) {
    return;
  }
  if (!ready) {
    queue.push(data instanceof Uint8Array ? data : new Uint8Array(data));
    return;
  }
  handle(data instanceof Uint8Array ? data : new Uint8Array(data));
};

function handle(bytes) {
  try {
    const out = worker_entry(bytes);
    // `worker_entry` returns a Uint8Array whose buffer is a view into the
    // wasm module's linear memory. We must NOT transfer that buffer (it
    // would detach the wasm heap). Copy into a standalone ArrayBuffer and
    // transfer the copy.
    const copy = new ArrayBuffer(out.byteLength);
    new Uint8Array(copy).set(out);
    self.postMessage(copy, [copy]);
  } catch (err) {
    // Re-throw so the main thread sees an onerror event; swallow otherwise
    // to avoid taking down the worker for a single bad request.
    console.error("worker_entry failed:", err);
  }
}

async function boot() {
  await init();
  ready = true;
  // Flush queued requests.
  while (queue.length) {
    handle(queue.shift());
  }
}

boot();