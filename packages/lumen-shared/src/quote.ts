const escapes: Readonly<Record<string, string>> = {
  "\\": "\\\\",
  "'": "\\'",
  "\n": "\\n",
  "\r": "\\r",
  "\t": "\\t",
};

/** `text` as a single-quoted expression string. */
export function quote(text: string): string {
  return `'${text.replace(/[\\'\n\r\t]/g, (char) => escapes[char] ?? char)}'`;
}
