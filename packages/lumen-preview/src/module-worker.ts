/**
 * Starts a module worker from `url`, resolved against the page. Browsers refuse a worker script on
 * another origin, such as a CDN, so that one starts from a same-origin blob that imports it, which
 * needs the script served with CORS.
 */
export function createModuleWorker(url: string | URL): Worker {
  const script = new URL(url, globalThis.document.baseURI);
  // `origin` is the document's effective origin, which an inherited `about:blank` or `srcdoc`
  // frame reports and its `location.origin` doesn't.
  if (
    script.origin === globalThis.origin ||
    script.protocol === "data:" ||
    script.protocol === "blob:"
  ) {
    return new Worker(script.href, { type: "module" });
  }
  const source = `import ${JSON.stringify(script.href)};`;
  const blobUrl = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
  const revoke = () => URL.revokeObjectURL(blobUrl);
  let worker: Worker;
  try {
    worker = new Worker(blobUrl, { type: "module" });
  } catch (error) {
    revoke();
    throw error;
  }
  // Some WebKit versions give the worker an opaque origin if the blob URL is revoked before it starts.
  worker.addEventListener("message", revoke, { once: true });
  worker.addEventListener("error", revoke, { once: true });
  return worker;
}
