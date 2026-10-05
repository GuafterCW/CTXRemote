/** Groups up to nine digits as "482 913 077" while typing. */
export function formatId(input: string): string {
  const digits = input.replace(/\D/g, "").slice(0, 9);
  return digits.replace(/(\d{3})(?=\d)/g, "$1 ");
}

export function isCompleteId(input: string): boolean {
  return input.replace(/\D/g, "").length === 9;
}

/** Mirrors `normalize_alias` in crates/proto/src/rendezvous.rs. */
export function isPublicAlias(input: string): boolean {
  const alias = input.trim().toLowerCase();
  return /^[a-z0-9](?:[a-z0-9._-]{1,30})[a-z0-9]$/.test(alias) && /[a-z]/.test(alias);
}

const relative = new Intl.RelativeTimeFormat("de", { numeric: "auto" });

export function since(unixSeconds: number): string {
  const diff = unixSeconds - Date.now() / 1000;
  const steps: [number, Intl.RelativeTimeFormatUnit][] = [
    [60, "second"],
    [60, "minute"],
    [24, "hour"],
    [7, "day"],
    [4.35, "week"],
    [12, "month"],
  ];
  let value = diff;
  for (const [size, unit] of steps) {
    if (Math.abs(value) < size) {
      return unit === "second" ? "gerade eben" : relative.format(Math.round(value), unit);
    }
    value /= size;
  }
  return relative.format(Math.round(value), "year");
}

const sizeNumber = new Intl.NumberFormat("de-DE", { maximumFractionDigits: 1 });
const UNITS = ["B", "KB", "MB", "GB", "TB"];

function unitFor(bytes: number): number {
  let i = 0;
  while (bytes >= 1024 ** (i + 1) && i < UNITS.length - 1) i++;
  return i;
}

/** "1,4 MB". */
export function formatSize(bytes: number): string {
  const i = unitFor(bytes);
  return `${sizeNumber.format(i === 0 ? Math.round(bytes) : bytes / 1024 ** i)} ${UNITS[i]}`;
}

/** "x MB von y MB", both in the unit of the total. */
export function formatProgress(done: number, total: number): string {
  const i = unitFor(total);
  const n = (v: number) => sizeNumber.format(i === 0 ? Math.round(v) : v / 1024 ** i);
  return `${n(done)} von ${n(total)} ${UNITS[i]}`;
}

/** "04.10.2026, 14:03", empty if unknown (0). */
export function formatDate(unixSeconds: number): string {
  if (!unixSeconds) return "";
  return new Intl.DateTimeFormat("de-DE", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(unixSeconds * 1000));
}
