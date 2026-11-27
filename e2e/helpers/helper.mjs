// The programs the end-to-end tests run inside the terminal.
//
//   node helper.mjs <command> <id> [arguments]
//
// Everything a command prints carries the <id> of its run, so that a test
// never mistakes the output of an earlier run, or the echo of the command
// line itself, for the one it is waiting for. A command line never contains
// its own marker: markers put the id right after the name, command lines put
// a space in between.
//
// Plain Node.js without dependencies: it is the one interpreter every
// machine that runs the suite has, and it behaves the same under bash on
// macOS and Linux and under cmd.exe on Windows.

const [command, id = "", ...rest] = process.argv.slice(2);

const write = (text) => process.stdout.write(text);
const line = (text) => write(`${text}\r\n`);

/** Writes `text` and waits until the terminal has taken it. */
function writeAll(text) {
  return new Promise((resolve) => {
    if (process.stdout.write(text)) resolve();
    else process.stdout.once("drain", resolve);
  });
}

function requireTerminal() {
  if (process.stdin.isTTY && process.stdout.isTTY) return;
  line(`NOT-A-TERMINAL-${id}`);
  process.exit(2);
}

const commands = {
  /**
   * Prints every byte it receives, in hexadecimal, as one growing line per
   * read: `RAW-<id>:1b0d:END`. With `--bracketed` it asks for bracketed
   * paste first. Ctrl+C (the byte 03) ends it.
   */
  raw() {
    requireTerminal();
    const bracketed = rest.includes("--bracketed");
    process.stdin.setRawMode(true);
    if (bracketed) write("\x1b[?2004h");
    let hex = "";
    process.stdin.on("data", (chunk) => {
      let done = false;
      for (const byte of chunk) {
        if (byte === 0x03) {
          done = true;
          break;
        }
        hex += byte.toString(16).padStart(2, "0");
      }
      if (hex !== "") line(`RAW-${id}:${hex}:END`);
      if (done) {
        if (bracketed) write("\x1b[?2004l");
        line(`RAW-EXIT-${id}`);
        process.exit(0);
      }
    });
    line(`RAW-READY-${id}`);
  },

  /**
   * Prints the size of the terminal as the process sees it, now and every
   * time it changes: `SIZE-<id>:120x40`. Any key ends it.
   */
  size() {
    requireTerminal();
    const print = () => {
      line(`SIZE-${id}:${String(process.stdout.columns)}x${String(process.stdout.rows)}`);
    };
    process.stdout.on("resize", print);
    process.stdin.setRawMode(true);
    process.stdin.on("data", () => {
      line(`SIZE-EXIT-${id}`);
      process.exit(0);
    });
    print();
  },

  /**
   * Prints its own process identifier and the one of the shell that started
   * it, then stays until it is killed: `HOLD-<id>:shell=12:self=34`.
   */
  hold() {
    line(`HOLD-${id}:shell=${String(process.ppid)}:self=${String(process.pid)}`);
    setInterval(() => undefined, 60_000);
  },

  /** Prints the UTF-8 text given in hexadecimal: `TEXT-<id>:<text>`. */
  text() {
    line(`TEXT-${id}:${Buffer.from(rest[0] ?? "", "hex").toString("utf8")}`);
  },

  /**
   * Asks the terminal to put the UTF-8 text given in hexadecimal on the
   * clipboard (OSC 52), then prints `COPY-<id>`.
   */
  copy() {
    const text = Buffer.from(rest[0] ?? "", "hex");
    write(`\x1b]52;c;${text.toString("base64")}\x07`);
    line(`COPY-${id}`);
  },

  /**
   * Prints, as JSON, the variables of its environment whose name starts
   * with `TERMX_`: `ENV-<id>:{"TERMX_LOG":"debug"}`.
   */
  env() {
    const entries = Object.entries(process.env).filter(([name]) =>
      name.toUpperCase().startsWith("TERMX_"),
    );
    line(`ENV-${id}:${JSON.stringify(Object.fromEntries(entries))}`);
  },

  /** Prints solid blocks (reverse video), then `PAINT-<id>`. */
  paint() {
    for (let row = 0; row < 6; row += 1) line(`\x1b[7m${" ".repeat(40)}\x1b[0m`);
    line(`PAINT-${id}`);
  },

  /**
   * Prints the given number of mebibytes as numbered lines, as fast as the
   * terminal takes them, then `BURST-<id>:lines=<count>`.
   */
  async burst() {
    const target = Number(rest[0] ?? "1") * 1024 * 1024;
    const filler = "x".repeat(60);
    let written = 0;
    let count = 0;
    while (written < target) {
      let block = "";
      for (let index = 0; index < 500; index += 1) {
        block += `burst ${String(count).padStart(8, "0")} ${filler}\r\n`;
        count += 1;
      }
      written += block.length;
      await writeAll(block);
    }
    await writeAll(`BURST-${id}:lines=${String(count)}\r\n`);
  },

  /**
   * Prints its process identifier, `TICKS-<id>:self=<pid>`, then
   * `TICK-<id>:<n>` for n from 1 to the count given, one every so many
   * milliseconds, then `TICKS-DONE-<id>`. A count of 0 ticks until Ctrl+C.
   */
  async ticks() {
    const count = Number(rest[0] ?? "10");
    const every = Number(rest[1] ?? "50");
    line(`TICKS-${id}:self=${String(process.pid)}`);
    for (let tick = 1; count === 0 || tick <= count; tick += 1) {
      await new Promise((resolve) => setTimeout(resolve, every));
      await writeAll(`TICK-${id}:${String(tick)}\r\n`);
    }
    line(`TICKS-DONE-${id}`);
  },

  /** Prints numbered lines for ever: `flood-<id> 12345`. Ctrl+C ends it. */
  async flood() {
    for (let count = 0; ; count += 500) {
      let block = "";
      for (let index = 0; index < 500; index += 1) {
        block += `flood-${id} ${String(count + index)}\r\n`;
      }
      await writeAll(block);
    }
  },
};

const run = commands[command];
if (!run) {
  line(`UNKNOWN-COMMAND-${id}`);
  process.exit(2);
}
await run();
