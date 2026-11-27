/** The id of the element of a sidebar row, to move the focus to it. */
export function rowElementId(sessionId: string): string {
  return `session-row-${sessionId}`;
}

/** The id of the element of a sidebar group. */
export function groupElementId(key: string): string {
  return `session-group-${encodeURIComponent(key)}`;
}
