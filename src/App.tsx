import { useCallback, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { OpenedElf, SymbolNode, inDesktopApp, openElf, startupElfPath } from "./elf/api";
import { SymbolTree } from "./elf/SymbolTree";
import { NodeDetails } from "./elf/NodeDetails";
import { watchableLeaves } from "./live/api";
import { ConnectBar } from "./live/ConnectBar";
import { LogConsole } from "./live/LogConsole";
import { Scope } from "./live/Scope";
import { StatusBar } from "./live/StatusBar";
import { WatchTable } from "./live/WatchTable";
import { useSession } from "./live/useSession";
import { useWatches } from "./live/useWatches";

function fileName(path: string) {
  return path.split(/[\\/]/).pop() ?? path;
}

export default function App() {
  const [elf, setElf] = useState<OpenedElf | null>(null);
  const [selected, setSelected] = useState<SymbolNode | null>(null);
  const [loading, setLoading] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const session = useSession();
  const watch = useWatches(elf);

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

  const { add } = watch;
  const onWatch = useCallback(
    (node: SymbolNode) => {
      if (node.kind === "scalar" || node.kind === "enum") {
        add([node]);
        return;
      }
      watchableLeaves(node.ref).then(
        (leaves) => {
          if (leaves.length === 0) setError(`${node.path} holds no numbers that can be sampled.`);
          else add(leaves);
        },
        (e) => setError(`Could not watch ${node.path}: ${e}`),
      );
    },
    [add],
  );

  const watchedPaths = useMemo(() => new Set(watch.watches.map((w) => w.path)), [watch.watches]);
  const connected = session.link.state === "connected";

  return (
    <div className="flex h-full flex-col">
      <header className="flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-rule bg-surface px-4 py-2">
        <button
          onClick={chooseElf}
          disabled={loading !== null}
          className="rounded-sm border border-rule bg-panel px-3 py-1 font-medium hover:bg-sunken disabled:opacity-60"
        >
          Open ELF…
        </button>
        {loading && <span className="text-muted">Reading {loading}…</span>}
        {!loading && elf && (
          <span className="font-mono text-[13px]" title={elf.summary.path}>
            {fileName(elf.summary.path)}
          </span>
        )}
        <ConnectBar
          link={session.link}
          canConnect={elf !== null}
          onConnect={(request) => void session.connect(request)}
          onDisconnect={() => void session.disconnect()}
        />
      </header>

      {error && (
        <div role="alert" className="flex items-center border-b border-rule bg-surface px-4 py-2 text-danger">
          <span className="flex-1">{error}</span>
          <button onClick={() => setError(null)} className="rounded-sm px-2 text-muted hover:bg-sunken hover:text-ink">
            Dismiss
          </button>
        </div>
      )}

      {elf ? (
        <main className="grid min-h-0 flex-1 grid-cols-[minmax(300px,30%)_1fr]">
          <section className="flex min-h-0 flex-col border-r border-rule">
            <div className="min-h-0 flex-1">
              <SymbolTree
                roots={elf.roots}
                selected={selected}
                onSelect={setSelected}
                onWatch={onWatch}
                watched={watchedPaths}
              />
            </div>
            <div className="max-h-[40%] shrink-0 overflow-auto border-t border-rule bg-surface">
              <NodeDetails node={selected} roots={elf.roots} onWatch={onWatch} />
            </div>
          </section>
          <section className="grid min-h-0 grid-rows-[minmax(0,3fr)_minmax(0,2fr)] bg-surface">
            <div className="min-h-0 border-b border-rule">
              <Scope watches={watch.watches} connected={connected} />
            </div>
            <div className="grid min-h-0 grid-cols-2">
              <div className="min-h-0 border-r border-rule">
                <WatchTable
                  watches={watch.watches}
                  onTogglePlot={watch.togglePlot}
                  onRemove={watch.remove}
                  onClear={watch.clear}
                />
              </div>
              <LogConsole
                lines={session.logs}
                stream={session.stats?.log ?? null}
                connected={connected}
                onClear={session.clearLogs}
              />
            </div>
          </section>
        </main>
      ) : (
        <main className="flex flex-1 items-center justify-center p-8">
          <div className="max-w-md">
            <h1 className="text-[20px] font-semibold">Open a firmware build to watch it live</h1>
            <p className="mt-2 leading-relaxed text-muted">
              Pick the ELF that cargo or your IDE produced, for example
              <span className="font-mono text-ink"> target/thumbv7em-none-eabihf/release/balance-infantry-chassis</span>.
              Then connect the debug probe to plot its statics and read its defmt log while it runs.
            </p>
          </div>
        </main>
      )}

      <StatusBar elf={elf} link={session.link} stats={session.stats} />
    </div>
  );
}
