/**
 * Starts a module worker from `url`, resolved against the page. Browsers refuse a worker script on
 * another origin, such as a CDN, so that one starts from a same-origin blob that imports it, which
 * needs the script served with CORS.
 */
export function createModuleWorker(url: string | URL): Worker {
  const script = new URL(url, globalThis.document.baseURI);
  if (script.origin === globalThis.location.origin) {
    return new Worker(script.href, { type: "module" });
  }
  const source = `import ${JSON.stringify(script.href)};`;
  const blobUrl = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
  try {
    return new Worker(blobUrl, { type: "module" });
  } finally {
    // The constructor has resolved the blob already, so the worker still loads it.
    URL.revokeObjectURL(blobUrl);
  }
}
