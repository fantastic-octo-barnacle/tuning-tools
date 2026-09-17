import { Scalar } from "../elf/api";

export function formatValue(value: number | undefined, scalar: Scalar | null): string {
  if (value === undefined) return "";
  if (Number.isNaN(value)) return "read failed";
  if (scalar === "bool") return value ? "true" : "false";
  if (scalar === "f32" || scalar === "f64") {
    if (value === 0) return "0";
    const abs = Math.abs(value);
    return abs >= 1e6 || abs < 1e-4 ? value.toExponential(4) : String(Number(value.toPrecision(6)));
  }
  return String(value);
}

export function formatRate(hz: number): string {
  return hz >= 100 ? hz.toFixed(0) : hz.toFixed(1);
}

export function formatMicros(us: number): string {
  return us >= 1000 ? `${(us / 1000).toFixed(1)} ms` : `${Math.round(us)} µs`;
}
