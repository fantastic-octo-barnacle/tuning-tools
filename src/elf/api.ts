import { invoke, isTauri } from "@tauri-apps/api/core";

export type Step =
  | { kind: "member"; value: string }
  | { kind: "index"; value: number }
  | { kind: "variant"; value: string }
  | { kind: "discriminant" };

export interface NodeRef {
  symbol: string;
  steps: Step[];
}

export type NodeKind =
  | "scalar"
  | "enum"
  | "taggedEnum"
  | "struct"
  | "union"
  | "array"
  | "pointer"
  | "function"
  | "other";

export type Scalar =
  | "u8" | "u16" | "u32" | "u64"
  | "i8" | "i16" | "i32" | "i64"
  | "f32" | "f64" | "bool"
  | { raw: number };

export interface SymbolNode {
  ref: NodeRef;
  label: string;
  path: string;
  address: number;
  size: number | null;
  typeName: string;
  kind: NodeKind;
  scalar: Scalar | null;
  expandable: boolean;
  childCount: number | null;
  readable: boolean;
  status: string | null;
  bitOffset: number | null;
  bitSize: number | null;
  discrValue: number | null;
}

export interface RootNode extends SymbolNode {
  segments: string[];
  section: string;
  readOnly: boolean;
}

export interface Children {
  nodes: SymbolNode[];
  total: number;
}

export interface ElfSummary {
  path: string;
  machine: string;
  is64bit: boolean;
  littleEndian: boolean;
  entryPoint: number;
  variables: number;
  functions: number;
  types: number;
  diagnostics: {
    totalVariables: number;
    withValidAddress: number;
    optimizedOut: number;
    localVariables: number;
    externDeclarations: number;
    compileTimeConstants: number;
    registerOnly: number;
  };
}

export interface OpenedElf {
  summary: ElfSummary;
  roots: RootNode[];
  parseMs: number;
}

export const inDesktopApp = isTauri;

export function openElf(path: string): Promise<OpenedElf> {
  return invoke("open_elf", { path });
}

export function symbolChildren(node: NodeRef, limit?: number): Promise<Children> {
  return invoke("symbol_children", { node, limit });
}

export function hex(n: number): string {
  return "0x" + n.toString(16).padStart(8, "0");
}

export function scalarName(s: Scalar): string {
  return typeof s === "string" ? s : `${s.raw} raw bytes`;
}

export function startupElfPath(): Promise<string | null> {
  return invoke("startup_elf_path");
}
