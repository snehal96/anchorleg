// Images pasted into a message box become attachments.
import type { ClipboardEvent } from "react";
import { savePaste } from "./api";

/** The pasted images saved as files, or null when the paste has none (it goes on as text). */
export function pastedImages(e: ClipboardEvent): Promise<string[]> | null {
  const files = [...e.clipboardData.items]
    .filter((i) => i.kind === "file" && i.type.startsWith("image/"))
    .map((i) => i.getAsFile())
    .filter((f): f is File => f != null);
  if (!files.length) return null;
  e.preventDefault();
  return Promise.all(files.map(async (f) => savePaste(new Uint8Array(await f.arrayBuffer()), f.type.split("/")[1]?.replace("jpeg", "jpg") || "png")));
}
