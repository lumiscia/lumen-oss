type PendingRead = {
  reject: (error: unknown) => void;
  resolve: (result: IteratorResult<number>) => void;
};

export class TimestampQueue implements AsyncIterable<number> {
  private closed = false;
  private pendingRead: PendingRead | null = null;
  private readonly values: number[] = [];

  push(value: number): void {
    if (this.closed) {
      throw new Error("timestamp queue is closed");
    }

    if (this.pendingRead) {
      const { resolve } = this.pendingRead;
      this.pendingRead = null;
      resolve({ done: false, value });
      return;
    }

    this.values.push(value);
  }

  close(error?: unknown): void {
    if (this.closed) {
      return;
    }

    this.closed = true;
    if (!this.pendingRead) {
      return;
    }

    const { reject, resolve } = this.pendingRead;
    this.pendingRead = null;
    if (error) {
      reject(error);
    } else {
      resolve({ done: true, value: undefined });
    }
  }

  [Symbol.asyncIterator](): AsyncIterator<number> {
    return {
      next: () => this.next(),
      return: async () => {
        this.close();
        return { done: true, value: undefined };
      },
    };
  }

  private next(): Promise<IteratorResult<number>> {
    const value = this.values.shift();
    if (value !== undefined) {
      return Promise.resolve({ done: false, value });
    }

    if (this.closed) {
      return Promise.resolve({ done: true, value: undefined });
    }

    return new Promise<IteratorResult<number>>((resolve, reject) => {
      this.pendingRead = { reject, resolve };
    });
  }
}
