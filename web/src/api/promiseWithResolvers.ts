/** Chrome 114 and older WebKit need the constructor-based fallback. */
export function promiseWithResolvers<T>(): PromiseWithResolvers<T> {
  if (typeof Promise.withResolvers === "function")
    return Promise.withResolvers<T>();

  let resolve: PromiseWithResolvers<T>["resolve"] = () => {};
  let reject: PromiseWithResolvers<T>["reject"] = () => {};
  const promise = new Promise<T>((accept, fail) => {
    resolve = accept;
    reject = fail;
  });
  return { promise, resolve, reject };
}
