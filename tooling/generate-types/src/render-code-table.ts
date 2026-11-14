import { type CodeEntry } from "./render-runtime.js";

const start = "<!-- generated:diagnostic-codes:start -->";
const end = "<!-- generated:diagnostic-codes:end -->";

/** The table of diagnostic codes, with its columns padded as `oxfmt` pads them. */
export function renderCodeTable(codes: readonly CodeEntry[]): string {
  const rows = [
    ["Code", "Severity", "Found by", "Meaning"],
    ...codes.map((entry) => [`\`${entry.code}\``, entry.severity, entry.foundBy, entry.meaning]),
  ];
  const widths = rows[0]!.map((_, column) => Math.max(...rows.map((row) => row[column]!.length)));
  const line = (cells: readonly string[]): string =>
    `| ${cells.map((cell, column) => cell.padEnd(widths[column]!)).join(" | ")} |`;
  const [header, ...body] = rows;
  return [line(header!), line(widths.map((width) => "-".repeat(width))), ...body.map(line)].join(
    "\n",
  );
}

/** `readme` with what is between the diagnostic code markers replaced by `table`, set apart by blank lines as `oxfmt` sets it. */
export function replaceCodeTable(readme: string, table: string): string {
  const from = readme.indexOf(start);
  const to = readme.indexOf(end);
  if (from === -1 || to < from) throw new Error(`the README lacks the ${start} … ${end} markers`);
  return `${readme.slice(0, from + start.length)}\n\n${table}\n\n${readme.slice(to)}`;
}
