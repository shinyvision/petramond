// Main thread loader: inits the llamacraft wasm module and calls `run(canvas)`.

import init, { run } from "./llamacraft.js";

async function main() {
  await init();
  const canvas = document.getElementById("canvas");
  run(canvas);
}

main().catch((e) => {
  console.error("llamacraft init failed:", e);
  const el = document.getElementById("err");
  if (el) el.textContent = String(e);
});