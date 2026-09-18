import { RootNode, SymbolNode, hex, scalarName } from "./api";
import { MemoryStrip } from "./MemoryStrip";
import { watchable } from "./SymbolTree";

const kindText: Record<SymbolNode["kind"], string> = {
  scalar: "Scalar",
  enum: "Enum",
  taggedEnum: "Enum with data",
  struct: "Struct",
  union: "Union",
  array: "Array",
  pointer: "Pointer (not followed)",
  function: "Function",
  other: "Other",
};

function Field({ name, children }: { name: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[6rem_1fr] gap-3 py-1">
      <dt className="text-muted">{name}</dt>
      <dd className="min-w-0 break-words">{children}</dd>
    </div>
  );
}

interface Props {
  node: SymbolNode | null;
  roots: RootNode[];
  onWatch: (node: SymbolNode) => void;
}

export function NodeDetails({ node, roots, onWatch }: Props) {
  if (!node) {
    return <p className="p-4 text-muted">Select a symbol to see where it lives and how to read it.</p>;
  }
  const root = roots.find((r) => r.path === node.ref.symbol);
  const size = node.size ?? 0;

  return (
    <article className="p-4">
      <div className="flex items-start gap-3">
        <h2 className="min-w-0 flex-1 font-mono text-[13px] leading-snug break-all select-text">{node.path}</h2>
        {watchable(node) && (
          <button
            onClick={() => onWatch(node)}
            className="shrink-0 rounded-sm border border-rule bg-panel px-2 py-0.5 hover:bg-sunken"
          >
            {node.expandable ? "Watch numbers inside" : "Watch"}
          </button>
        )}
      </div>
      {!node.readable && (
        <p className="mt-2 text-danger">Cannot be read from the target: {node.status ?? "no valid address"}.</p>
      )}

      <MemoryStrip node={node} roots={roots} />

      <dl className="mt-4 divide-y divide-rule border-y border-rule font-mono text-[12px] select-text">
        <Field name="Address">
          {hex(node.address)}
          {size > 1 && <span className="text-muted"> to {hex(node.address + size - 1)}</span>}
        </Field>
        <Field name="Size">{node.size === null ? "unknown" : `${size} ${size === 1 ? "byte" : "bytes"}`}</Field>
        <Field name="Type">{node.typeName}</Field>
        {node.wrapper && <Field name="Wrapped in">{node.wrapper}</Field>}
        <Field name="Kind">{kindText[node.kind]}</Field>
        {node.scalar && <Field name="Read as">{scalarName(node.scalar)}</Field>}
        {node.bitSize !== null && (
          <Field name="Bitfield">
            {node.bitSize} bits at bit {node.bitOffset ?? 0}
          </Field>
        )}
        {node.discrValue !== null && <Field name="Tag value">{node.discrValue}</Field>}
        {node.childCount !== null && node.kind !== "scalar" && (
          <Field name={node.kind === "array" ? "Elements" : node.kind === "taggedEnum" ? "Entries" : "Members"}>
            {node.childCount}
          </Field>
        )}
        {root && <Field name="Section">{root.section || "none"}</Field>}
      </dl>
    </article>
  );
}
