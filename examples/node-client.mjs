// A tiny client of the rust-toast program (PROTOCOL.md), with no
// dependencies: it spawns `rust-toast serve`, posts a card with two
// buttons and prints every event until the card is gone.
//
//   cargo build --features cli
//   node examples/node-client.mjs [path/to/rust-toast]
//
// The program exits by itself when its stdin closes, so it never outlives
// the app that spawned it.

import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

const exe = process.platform === "win32" ? "rust-toast.exe" : "rust-toast";
const bin = process.argv[2] ?? process.env.RUST_TOAST_BIN ?? `target/debug/${exe}`;

const child = spawn(bin, ["serve"], {
  stdio: ["pipe", "pipe", "inherit"],
  windowsHide: true,
});
child.on("error", (e) => {
  console.error(`can't start ${bin}: ${e.message}`);
  process.exit(1);
});
child.on("exit", (code) => console.log(`rust-toast exited (${code})`));

const send = (cmd) => child.stdin.write(JSON.stringify(cmd) + "\n");

createInterface({ input: child.stdout }).on("line", (line) => {
  let ev;
  try {
    ev = JSON.parse(line);
  } catch {
    console.error(`not JSON: ${line}`);
    return;
  }
  console.log(ev);
  switch (ev.event) {
    case "ready":
      if (!ev.supported) {
        console.log(`no cards on ${ev.platform}`);
        child.stdin.end();
        return;
      }
      send({ cmd: "configure", app_name: "Node client" });
      send({
        cmd: "post",
        id: "update",
        title: "An update is ready",
        body: "Restart now, or it installs when you quit.",
        kind: "info",
        sticky: true,
        actions: [
          { id: "restart", label: "Restart" },
          { id: "later", label: "Later" },
        ],
      });
      break;
    case "action":
    case "click":
    case "dismiss":
      if (ev.id === "update") child.stdin.end(); // done: the program exits
      break;
  }
});

// Give up after a minute; closing stdin ends the program and its cards.
setTimeout(() => child.stdin.end(), 60_000).unref();
