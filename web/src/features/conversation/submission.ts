import { commandText } from "./console.ts";

export interface ComposerSubmission {
  markdown?: boolean;
  text: string;
  draft: string;
  selected: string;
  consoleView: boolean;
  peer: string | null;
  replyId: string | null;
}

/** Capture before confirmation so subsequent edits or navigation cannot retarget it. */
export function captureSubmission(
  value: ComposerSubmission,
): Readonly<ComposerSubmission> {
  return Object.freeze({ ...value });
}
export function deletionKind(text: string): "message" | "resource" | null {
  const command = commandText(text).split(/\s+/u)[0];
  return command === "/rm"
    ? "resource"
    : command === "/retract"
      ? "message"
      : null;
}
export function submissionFrame(submission: Readonly<ComposerSubmission>) {
  const { text, peer, replyId, selected, consoleView } = submission;
  const message = !text.startsWith("/");
  return {
    ...(submission.markdown === undefined
      ? {}
      : { markdown: submission.markdown }),
    room: consoleView ? null : selected,
    text:
      message && replyId
        ? `/reply ${replyId} ${text}`
        : message && peer
          ? `/tell ${peer} ${text}`
          : text,
  };
}
