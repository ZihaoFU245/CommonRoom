import test from "node:test";
import assert from "node:assert/strict";
import { ApiError } from "../src/api/client.ts";
import { saveErrorMessage } from "../src/features/preferences/saveFeedback.ts";

test("save feedback explains connection/session failures and hides server diagnostics", () => {
  const storage = new ApiError(
    503,
    "Storage unavailable: database writer failed.",
  );
  assert.equal(
    saveErrorMessage(storage),
    "Couldn't save your changes right now. Please try again.",
  );
  assert.equal(storage.message, "Storage unavailable: database writer failed.");
  assert.equal(
    saveErrorMessage(new Error("Storage unavailable; rename was not applied.")),
    "Couldn't save your changes right now. Please try again.",
  );
  assert.equal(
    saveErrorMessage(new ApiError(401, "Unauthorized")),
    "Your session has expired. Sign in again.",
  );
  assert.equal(
    saveErrorMessage(new TypeError("Failed to fetch")),
    "Couldn't connect to the server. Check your connection and try again.",
  );
  assert.equal(
    saveErrorMessage(new DOMException("request deadline", "TimeoutError")),
    "The request took too long. Please try again.",
  );
  for (const [status, message] of [
    [400, "Choose a valid color."],
    [403, "You no longer have permission to change this color."],
    [409, "Username already exists."],
  ])
    assert.equal(saveErrorMessage(new ApiError(status, message)), message);
});
