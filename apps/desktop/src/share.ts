// Copying a task out of the app: one message, or the whole conversation as Markdown.
import type { Session } from "./api";

export const copyText = (text: string) => navigator.clipboard.writeText(text);

/** The conversation as Markdown: your messages, the agent's replies and the steps it took. */
export function conversationMarkdown(view: Session, title: string): string {
  const out = [`# ${title}`, "", `Folder: \`${view.run.cwd}\``, ""];
  for (const m of view.messages) {
    if (m.role === "user") out.push(`**You:** ${m.text}`, "");
    else if (m.role === "agent") out.push(`**Agent:** ${m.text}`, "");
    else if (m.role === "tool") out.push(`- ${m.text.replace(/ {2,}/g, " ")}`);
    else if (m.role === "error") out.push(`> Error: ${m.text}`, "");
  }
  return out.join("\n").replace(/\n{3,}/g, "\n\n").trim() + "\n";
}
