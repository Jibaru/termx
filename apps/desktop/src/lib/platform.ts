/** The operating systems termx runs on, as far as key bindings differ. */
export type Platform = "macos" | "windows" | "linux";

/** The platform the webview runs on. */
export function detectPlatform(navigatorLike: Pick<Navigator, "userAgent"> = navigator): Platform {
  const userAgent = navigatorLike.userAgent;
  if (/Mac/i.test(userAgent)) return "macos";
  if (/Win/i.test(userAgent)) return "windows";
  return "linux";
}
