import { RootNode, SymbolNode, hex, scalarName } from "./api";
import { MemoryStrip } from "./MemoryStrip";

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
    <div className="grid grid-cols-[7.5rem_1fr] gap-3 py-1.5">
      <dt className="text-muted">{name}</dt>
      <dd className="min-w-0 break-words">{children}</dd>
    </div>
  );
}

export function NodeDetails({ node, roots }: { node: SymbolNode | null; roots: RootNode[] }) {
  if (!node) {
    return <p className="p-8 text-muted">Select a symbol to see where it lives and how to read it.</p>;
  }
  const root = roots.find((r) => r.path === node.ref.symbol);
  const size = node.size ?? 0;

  return (
    <article className="max-w-3xl p-8">
      <h2 className="font-mono text-[15px] leading-snug break-all select-text">{node.path}</h2>
      {!node.readable && (
        <p className="mt-2 text-danger">Cannot be read from the target: {node.status ?? "no valid address"}.</p>
      )}

      <MemoryStrip node={node} roots={roots} />

      <dl className="mt-6 divide-y divide-rule border-y border-rule font-mono text-[12px] select-text">
        <Field name="Address">
          {hex(node.address)}
          {size > 1 && <span className="text-muted"> to {hex(node.address + size - 1)}</span>}
        </Field>
        <Field name="Size">{node.size === null ? "unknown" : `${size} ${size === 1 ? "byte" : "bytes"}`}</Field>
        <Field name="Type">{node.typeName}</Field>
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
