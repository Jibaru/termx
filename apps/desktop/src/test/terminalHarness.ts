import type { SessionHandlers, SessionId, StartedSession, StartSessionOptions } from "../lib/ipc";
import type { TerminalBackend } from "../terminal/TerminalController";

/**
 * What the tests of the terminal view share: a stand-in for the Rust side
 * that a test answers when it wants to, and panes with a size (happy-dom
 * does no layout).
 */

export const ACK_BATCH = 1000;

/** The session the terminals of the tests show. */
export const SESSION = "s-test";

/** A promise settled by the test. */
export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** One call to `startSession`, which the test answers when it wants to. */
export interface CreateCall {
  /** The session that was started. */
  session: SessionId;
  options: StartSessionOptions;
  handlers: SessionHandlers;
  /** Resolves the call: the session runs in a terminal. */
  started: (overrides?: Partial<StartedSession>) => Promise<void>;
  /** Rejects the call. */
  failed: (reason: unknown) => Promise<void>;
}

/** The Rust side, as far as a controller can tell. */
export function fakeBackend() {
  const creates: CreateCall[] = [];
  const writes: { id: number; data: string; binary: boolean; done: () => Promise<void> }[] = [];
  const resizes: { id: number; cols: number; rows: number }[] = [];
  const acks: { id: number; bytes: number }[] = [];
  const titles: { id: SessionId; title: string }[] = [];
  const folders: { id: SessionId; cwd: string }[] = [];
  const opened: string[] = [];
  const clipboard = { text: "", writes: [] as string[], reads: 0 };
  let nextId = 1;

  const backend: TerminalBackend = {
    startSession: (session, options, handlers) => {
      const call = deferred<StartedSession>();
      const terminal = nextId++;
      creates.push({
        session,
        options,
        handlers,
        started: async (overrides = {}) => {
          call.resolve({
            terminal,
            ackBatchBytes: ACK_BATCH,
            windowsBuildNumber: null,
            ...overrides,
          });
          await settle();
        },
        failed: async (reason) => {
          call.reject(reason);
          await settle();
        },
      });
      return call.promise;
    },
    writeSession: (id, data, binary = false) => {
      const call = deferred<undefined>();
      writes.push({
        id,
        data,
        binary,
        done: async () => {
          call.resolve(undefined);
          await settle();
        },
      });
      return call.promise;
    },
    resizeSession: (id, cols, rows) => {
      resizes.push({ id, cols, rows });
      return Promise.resolve();
    },
    ackSession: (id, bytes) => {
      acks.push({ id, bytes });
      return Promise.resolve();
    },
    reportSessionTitle: (id, title) => {
      titles.push({ id, title });
      return Promise.resolve();
    },
    reportSessionCwd: (id, cwd) => {
      folders.push({ id, cwd });
      return Promise.resolve();
    },
    openExternalUrl: (url) => {
      opened.push(url);
      return Promise.resolve();
    },
    readClipboardText: () => {
      clipboard.reads += 1;
      return Promise.resolve(clipboard.text);
    },
    writeClipboardText: (text) => {
      clipboard.writes.push(text);
      return Promise.resolve();
    },
  };
  return { backend, creates, writes, resizes, acks, titles, folders, opened, clipboard };
}

/** Lets the promise callbacks that are ready run. */
export async function settle(): Promise<void> {
  for (let turn = 0; turn < 5; turn += 1) await Promise.resolve();
}

/** A pane of the given size. happy-dom does no layout, so the size is set. */
export function pane(width = 960, height = 600): HTMLElement {
  const element = document.createElement("div");
  Object.defineProperty(element, "clientWidth", { configurable: true, value: width });
  Object.defineProperty(element, "clientHeight", { configurable: true, value: height });
  document.body.appendChild(element);
  return element;
}

export function resizePane(element: HTMLElement, width: number, height: number): void {
  Object.defineProperty(element, "clientWidth", { configurable: true, value: width });
  Object.defineProperty(element, "clientHeight", { configurable: true, value: height });
}

export function bytes(text: string): Uint8Array {
  return new TextEncoder().encode(text);
}
