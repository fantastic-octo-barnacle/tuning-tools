import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { OpenedElf, SymbolNode, inDesktopApp, openElf, startupElfPath } from "./elf/api";
import { SymbolTree } from "./elf/SymbolTree";
import { NodeDetails } from "./elf/NodeDetails";

function fileName(path: string) {
  return path.split(/[\\/]/).pop() ?? path;
}

export default function App() {
  const [elf, setElf] = useState<OpenedElf | null>(null);
  const [selected, setSelected] = useState<SymbolNode | null>(null);
  const [loading, setLoading] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function loadElf(path: string) {
    setLoading(fileName(path));
    setError(null);
    try {
      const opened = await openElf(path);
      setElf(opened);
      setSelected(null);
    } catch (e) {
      setError(`Could not open ${fileName(path)}: ${e}`);
    } finally {
      setLoading(null);
    }
  }

  async function chooseElf() {
    if (!inDesktopApp()) {
      setError("Opening firmware needs the desktop app. Start it with npm run tauri dev.");
      return;
    }
    const path = await open({ title: "Open firmware ELF", multiple: false, directory: false });
    if (typeof path === "string") await loadElf(path);
  }

  useEffect(() => {
    if (!inDesktopApp()) return;
    startupElfPath().then((path) => {
      if (path) void loadElf(path);
    });
  }, []);

  const ramStatics = elf?.roots.filter((r) => !r.readOnly).length ?? 0;

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-4 border-b border-rule bg-surface px-4 py-2.5">
        <button
          onClick={chooseElf}
          disabled={loading !== null}
          className="rounded-sm border border-rule bg-panel px-3 py-1 font-medium hover:bg-sunken disabled:opacity-60"
        >
          Open ELF…
        </button>
        {loading && <span className="text-muted">Reading {loading}…</span>}
        {!loading && elf && (
          <>
            <span className="font-mono text-[13px]" title={elf.summary.path}>
              {fileName(elf.summary.path)}
            </span>
            <dl className="ml-auto flex gap-5 text-[12px] text-muted">
              <div className="flex gap-1.5"><dt>Target</dt><dd className="text-ink">{elf.summary.machine}</dd></div>
              <div className="flex gap-1.5"><dt>RAM statics</dt><dd className="text-ink tabular-nums">{ramStatics}</dd></div>
              <div className="flex gap-1.5"><dt>Types</dt><dd className="text-ink tabular-nums">{elf.summary.types}</dd></div>
              <div className="flex gap-1.5"><dt>Parsed in</dt><dd className="text-ink tabular-nums">{elf.parseMs} ms</dd></div>
            </dl>
          </>
        )}
      </header>

      {error && (
        <div role="alert" className="border-b border-rule bg-surface px-4 py-2 text-danger">
          {error}
        </div>
      )}

      {elf ? (
        <main className="grid min-h-0 flex-1 grid-cols-[minmax(320px,40%)_1fr]">
          <section className="min-h-0 border-r border-rule">
            <SymbolTree roots={elf.roots} selected={selected} onSelect={setSelected} />
          </section>
          <section className="min-h-0 overflow-auto bg-surface">
            <NodeDetails node={selected} roots={elf.roots} />
          </section>
        </main>
      ) : (
        <main className="flex flex-1 items-center justify-center p-8">
          <div className="max-w-md">
            <h1 className="text-[20px] font-semibold">Open a firmware build to browse its statics</h1>
            <p className="mt-2 leading-relaxed text-muted">
              Pick the ELF that cargo or your IDE produced, for example
              <span className="font-mono text-ink"> target/thumbv7em-none-eabihf/release/balance-infantry-chassis</span>.
              Symbols are listed by their module path with the types the compiler recorded.
            </p>
          </div>
        </main>
      )}
    </div>
  );
}
