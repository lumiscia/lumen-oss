export function toUint8Array(value: BufferSource): Uint8Array {
  if (value instanceof Uint8Array) {
    return new Uint8Array(value);
  }

  if (value instanceof ArrayBuffer) {
    return new Uint8Array(value);
  }

  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength).slice();
  }

  throw new TypeError("expected a Uint8Array, ArrayBuffer, or typed array view");
}
