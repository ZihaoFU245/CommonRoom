import test from "node:test";
import assert from "node:assert/strict";
import { resizeLibraryImage } from "../src/features/conversation/resizeLibraryImage.ts";

function jpeg(size = 2000) {
  const bytes = new Uint8Array(size);
  bytes.set([255, 216, 255]);
  return new File([bytes], "photo.jpeg", {
    type: "image/jpeg",
    lastModified: 123,
  });
}

function png(animated) {
  const chunks = animated
    ? ["IHDR", "acTL", "IDAT", "IEND"]
    : ["IHDR", "IDAT", "IEND"];
  const bytes = new Uint8Array(8 + chunks.length * 12);
  bytes.set([137, 80, 78, 71, 13, 10, 26, 10]);
  chunks.forEach((tag, index) =>
    bytes.set(
      [...tag].map((c) => c.charCodeAt(0)),
      12 + index * 12,
    ),
  );
  return new File([bytes], "sticker.png", { type: "image/png" });
}

function webp(animated) {
  const bytes = new Uint8Array(30);
  bytes.set([..."RIFF"].map((c) => c.charCodeAt(0)));
  new DataView(bytes.buffer).setUint32(4, 22, true);
  bytes.set(
    [..."WEBPVP8X"].map((c) => c.charCodeAt(0)),
    8,
  );
  new DataView(bytes.buffer).setUint32(16, 10, true);
  bytes[20] = animated ? 2 : 0;
  return new File([bytes], "sticker.webp", { type: "image/webp" });
}

function browser(
  t,
  { width = 3840, height = 2160, transparent = false, encode } = {},
) {
  const saved = Object.fromEntries(
    ["Image", "document"].map((key) => [
      key,
      Object.getOwnPropertyDescriptor(globalThis, key),
    ]),
  );
  t.after(() => {
    for (const [key, descriptor] of Object.entries(saved)) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  const attempts = [];
  const canvas = {
    width: 0,
    height: 0,
    getContext: () => ({
      drawImage() {},
      getImageData: () => ({
        data: new Uint8ClampedArray([20, 40, 60, transparent ? 100 : 255]),
      }),
    }),
    toBlob(callback, type, quality) {
      attempts.push({ width: this.width, height: this.height, type, quality });
      if (encode) encode(callback, type);
      else callback(new Blob([new Uint8Array(100)], { type }));
    },
  };
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: { createElement: () => canvas },
  });
  Object.defineProperty(globalThis, "Image", {
    configurable: true,
    value: class {
      naturalWidth = width;
      naturalHeight = height;
      set src(value) {
        if (value) queueMicrotask(() => this.onload?.());
      }
    },
  });
  const revoked = t.mock.method(URL, "revokeObjectURL", () => {});
  return { attempts, canvas, revoked };
}

test("library encoding preserves aspect ratio and releases temporary resources", async (t) => {
  const { attempts, canvas, revoked } = browser(t);
  const file = jpeg();
  const output = await resizeLibraryImage(file, new AbortController().signal);
  assert.deepEqual(attempts, [
    { width: 1920, height: 1080, type: "image/webp", quality: 0.85 },
  ]);
  assert.equal(output.name, "photo.webp");
  assert.equal(output.type, "image/webp");
  assert.equal(output.size, 100);
  assert.equal(output.lastModified, file.lastModified);
  assert.equal(canvas.width, 0);
  assert.equal(revoked.mock.callCount(), 1);
});

test("small images keep original bytes when encoding would grow them", async (t) => {
  const { attempts } = browser(t, { width: 100, height: 200 });
  const file = jpeg(50);
  assert.equal(
    await resizeLibraryImage(file, new AbortController().signal),
    file,
  );
  assert.equal(attempts[0].width, 100);
  assert.equal(attempts[0].height, 200);
});

for (const transparent of [true, false])
  test(`Safari PNG fallback preserves ${transparent ? "transparency" : "opaque photo compression"}`, async (t) => {
    const { attempts } = browser(t, {
      transparent,
      encode(callback, type) {
        callback(
          new Blob([new Uint8Array(100)], {
            type: type === "image/webp" ? "image/png" : type,
          }),
        );
      },
    });
    const output = await resizeLibraryImage(
      jpeg(),
      new AbortController().signal,
    );
    assert.equal(output.type, transparent ? "image/png" : "image/jpeg");
    assert.equal(output.name, transparent ? "photo.png" : "photo.jpg");
    assert.equal(attempts.length, transparent ? 1 : 2);
  });

test("animation and unknown containers are preserved without image decoding", async () => {
  for (const file of [
    png(true),
    webp(true),
    new File(["GIF89a"], "wave.gif"),
    new File(["avif"], "motion.avif"),
    new File(["<svg/>"], "icon.svg"),
    new File(["RIFFbrokenWEBP"], "broken.webp"),
  ])
    assert.equal(
      await resizeLibraryImage(file, new AbortController().signal),
      file,
    );
});

test("static PNG and WebP are eligible without trusting their declared MIME", async (t) => {
  browser(t);
  for (const file of [png(false), webp(false)]) {
    const output = await resizeLibraryImage(file, new AbortController().signal);
    assert.notEqual(output, file);
    assert.equal(output.type, "image/webp");
  }
});

test("aborted encoding cannot produce an upload and releases temporary resources", async (t) => {
  const abort = new AbortController();
  const { canvas, revoked } = browser(t, {
    encode(callback) {
      abort.abort();
      callback(new Blob(["encoded"], { type: "image/webp" }));
    },
  });
  await assert.rejects(resizeLibraryImage(jpeg(), abort.signal), {
    name: "AbortError",
  });
  assert.equal(canvas.width, 0);
  assert.equal(revoked.mock.callCount(), 1);
});

test("excessive pixel counts do not upload originals silently", async (t) => {
  browser(t, { width: 10000, height: 10000 });
  await assert.rejects(
    resizeLibraryImage(jpeg(), new AbortController().signal),
    /40 megapixels/u,
  );
});

test("failed encoders do not upload originals silently", async (t) => {
  browser(t, { encode: (callback) => callback(null) });
  await assert.rejects(
    resizeLibraryImage(jpeg(), new AbortController().signal),
    /Unable to encode/u,
  );
});
