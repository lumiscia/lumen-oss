import { expect, test } from "vitest";

import { TimestampQueue } from "../src/media/timestamp-queue.ts";

test("timestamp queue yields pushed values in order", async () => {
  const queue = new TimestampQueue();
  const iterator = queue[Symbol.asyncIterator]();

  queue.push(1.25);
  queue.push(2.5);

  await expect(iterator.next()).resolves.toEqual({ done: false, value: 1.25 });
  await expect(iterator.next()).resolves.toEqual({ done: false, value: 2.5 });
});

test("timestamp queue wakes a pending read", async () => {
  const queue = new TimestampQueue();
  const iterator = queue[Symbol.asyncIterator]();
  const pending = iterator.next();

  queue.push(3.75);

  await expect(pending).resolves.toEqual({ done: false, value: 3.75 });
});

test("timestamp queue closes pending and future reads", async () => {
  const queue = new TimestampQueue();
  const iterator = queue[Symbol.asyncIterator]();
  const pending = iterator.next();

  queue.close();

  await expect(pending).resolves.toEqual({ done: true, value: undefined });
  await expect(iterator.next()).resolves.toEqual({ done: true, value: undefined });
});
