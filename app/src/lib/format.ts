/** Groups up to nine digits as "482 913 077" while typing. */
export function formatId(input: string): string {
  const digits = input.replace(/\D/g, "").slice(0, 9);
  return digits.replace(/(\d{3})(?=\d)/g, "$1 ");
}

export function isCompleteId(input: string): boolean {
  return input.replace(/\D/g, "").length === 9;
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
