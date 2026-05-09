import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

export async function writeGeneratedFile(filePath: string, content: string): Promise<void> {
  await mkdir(path.dirname(filePath), { recursive: true });
  await writeFile(filePath, content);
}
