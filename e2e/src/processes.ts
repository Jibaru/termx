/** Whether a process with this identifier exists. */
export function isAlive(pid: number): boolean {
  try {
    // Signal 0 sends nothing: it only checks that the process is there.
    process.kill(pid, 0);
    return true;
  } catch (error) {
    // Not allowed to signal it, so it exists.
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}
