import test from "node:test";
import assert from "node:assert/strict";
import { createRequestLifecycle } from "../src/features/conversation/requestLifecycle.ts";
import {
  captureSubmission,
  deletionKind,
  submissionFrame,
} from "../src/features/conversation/submission.ts";
import { expandAlias } from "../src/features/conversation/aliases.ts";

function fixture(transmit) {
  const frames = [],
    pending = [],
    results = [];
  const lifecycle = createRequestLifecycle(
    transmit ?? ((frame) => frames.push(frame)),
    (value) => pending.push(value),
  );
  const submit = (
    text = "hello",
    complete = (ack, request) => results.push({ ack, request }),
  ) =>
    lifecycle.submit({
      command: { text, room: "lobby", key: null },
      frame: { text, room: "lobby" },
      complete,
    });
  return { lifecycle, submit, frames, pending, results };
}

test("one pending request ignores unrelated and duplicate acknowledgements", () => {
  const f = fixture();
  assert.equal(f.submit(), true);
  assert.equal(f.submit("second"), false);
  f.lifecycle.receive({ kind: "notice", id: 99, text: "unrelated" });
  assert.equal(f.lifecycle.pending, true);
  assert.equal(f.results.length, 0);
  const ack = { kind: "notice", id: f.frames[0].id, text: "sent" };
  f.lifecycle.receive(ack);
  f.lifecycle.receive(ack);
  assert.equal(f.lifecycle.pending, false);
  assert.deepEqual(f.pending, [true, false]);
  assert.equal(f.results.length, 1);
  assert.equal(f.results[0].request.text, "hello");
  f.submit("second");
  assert.ok(f.frames[1].id > ack.id);
  f.lifecycle.receive(ack);
  assert.equal(f.lifecycle.pending, true);
});

test("server rejection completes once and permits the next operation", () => {
  const f = fixture();
  f.submit("/rm room protected");
  const ack = {
    kind: "error",
    id: f.frames[0].id,
    text: "Permission required",
  };
  f.lifecycle.receive(ack);
  assert.deepEqual(f.results[0].ack, ack);
  assert.equal(f.lifecycle.pending, false);
  assert.equal(f.submit("retry"), true);
});

for (const text of [
  "hello",
  "/react m1 🙂",
  "/edit m1 updated",
  "/retract m1",
  "/passwd secret",
]) {
  test(`send failures release the shared lifecycle for ${text.split(" ")[0]}`, async () => {
    for (const failure of [new Error("send failed"), "unknown failure"]) {
      const f = fixture(() => {
        throw failure;
      });
      const result = new Promise((resolve) => f.submit(text, resolve));
      const ack = await result;
      assert.equal(ack.kind, "error");
      assert.equal(
        ack.text,
        failure instanceof Error
          ? failure.message
          : "Command could not be sent.",
      );
      assert.equal(f.lifecycle.pending, false);
      assert.deepEqual(f.pending, [true, false]);
    }
  });
}

test("disconnect completes output and account promises without resending", async () => {
  const f = fixture();
  f.submit("/help");
  f.lifecycle.disconnect();
  f.lifecycle.disconnect();
  assert.equal(f.results.length, 1);
  assert.match(f.results[0].ack.text, /Check history/);
  assert.equal(f.frames.length, 1);
  const result = new Promise((resolve) => f.submit("/rename alice", resolve));
  const id = f.frames[1].id;
  f.lifecycle.disconnect();
  assert.equal((await result).kind, "error");
  f.lifecycle.receive({ kind: "notice", id, text: "late" });
  assert.equal(f.lifecycle.pending, false);
  assert.equal(f.frames.length, 2);
});

test("completion can start a new operation after pending is cleared", () => {
  const f = fixture();
  f.submit("first", () => assert.equal(f.submit("second"), true));
  f.lifecycle.receive({ kind: "notice", id: f.frames[0].id, text: "done" });
  assert.equal(f.lifecycle.pending, true);
  assert.equal(f.frames[1].text, "second");
});

test("confirmation captures alias expansion, sudo target and originating view", () => {
  const aliases = { "/drop": "/rm room" };
  const context = {
    text: expandAlias("/sudo /drop lobby", aliases),
    draft: "/sudo /drop lobby",
    selected: "lobby",
    consoleView: false,
    peer: null,
    replyId: null,
  };
  const captured = captureSubmission(context);
  context.text = "/rm room other";
  context.selected = "other";
  aliases["/drop"] = "/rm user";
  assert.deepEqual(submissionFrame(captured), {
    room: "lobby",
    text: "/sudo /rm room lobby",
  });
  assert.equal(deletionKind(captured.text), "resource");
  assert.equal(deletionKind("/sudo /retract m1"), "message");
  assert.equal(deletionKind("/react m1 🙂"), null);
  const f = fixture();
  // Nothing is transmitted until the captured operation is confirmed.
  assert.equal(f.frames.length, 0);
  f.lifecycle.submit({
    command: { text: captured.text, room: captured.selected, key: "local" },
    frame: submissionFrame(captured),
    complete: () => {},
  });
  assert.equal(f.frames[0].room, "lobby");
  assert.equal(f.frames[0].text, "/sudo /rm room lobby");
});

test("private history keeps current syntax and messages/replies retain Unicode", () => {
  const context = {
    text: "/history 1 100",
    draft: "/history 1 100",
    selected: "@direct:bob",
    consoleView: false,
    peer: "bob",
    replyId: null,
  };
  assert.deepEqual(submissionFrame(context), {
    room: "@direct:bob",
    text: "/history 1 100",
  });
  context.text = "/history";
  assert.equal(submissionFrame(context).text, "/history");
  context.text = "中文 🙂";
  assert.equal(submissionFrame(context).text, "/tell bob 中文 🙂");
  context.replyId = "m1";
  assert.equal(submissionFrame(context).text, "/reply m1 中文 🙂");
  context.consoleView = true;
  assert.equal(submissionFrame(context).room, null);
});
