import { OpenedElf } from "../elf/api";
import { CoreState, Stats } from "./api";
import { formatMicros, formatRate } from "./format";
import { Link } from "./useSession";

const coreText: Record<CoreState, string> = {
  running: "Running",
  halted: "Halted",
  sleeping: "Sleeping",
  lockedUp: "Locked up",
  unknown: "Unknown",
};

const linkText = {
  idle: "Not connected",
  connecting: "Connecting…",
  connected: "Connected",
  disconnected: "Disconnected",
  failed: "Connection failed",
};

function Cell({ children, title, tone }: { children: React.ReactNode; title?: string; tone?: "danger" | "warn" }) {
  return (
    <span
      title={title}
      className={`shrink-0 border-l border-rule px-3 py-[3px] tabular-nums ${tone === "danger" ? "text-danger" : tone === "warn" ? "text-warn" : ""}`}
    >
      {children}
    </span>
  );
}

export function StatusBar({ elf, link, stats }: { elf: OpenedElf | null; link: Link; stats: Stats | null }) {
  const ramStatics = elf?.roots.filter((r) => !r.readOnly).length ?? 0;
  const slow = stats && stats.targetHz > 0 && stats.achievedHz < stats.targetHz * 0.9;
  const lit = link.state === "connected";

  return (
    <footer data-statusbar className="flex items-stretch overflow-hidden border-t border-rule bg-surface text-[12px] whitespace-nowrap text-muted">
      {elf && (
        <span className="min-w-0 truncate px-3 py-[3px]" title={elf.summary.path}>
          {elf.summary.machine}, {ramStatics} RAM statics, parsed in {elf.parseMs} ms
        </span>
      )}
      <span className="ml-auto flex min-w-0 items-center gap-1.5 border-l border-rule px-3 py-[3px]">
        <span
          aria-hidden
          data-lit={lit || undefined}
          className={`h-[7px] w-[7px] shrink-0 rounded-full ${lit ? "bg-accent" : link.state === "failed" ? "bg-danger" : "bg-faint"}`}
        />
        <span className={link.state === "failed" ? "text-danger" : "text-ink"}>{linkText[link.state]}</span>
        {link.message && (
          <span className="min-w-0 truncate text-danger" title={link.message}>
            {link.message}
          </span>
        )}
      </span>
      {lit && stats && (
        <>
          <Cell title="Achieved sample rate over the last 200 ticks" tone={slow ? "warn" : undefined}>
            {stats.targetHz > 0 ? `${formatRate(stats.achievedHz)} of ${stats.targetHz} Hz` : "Idle"}
          </Cell>
          {stats.values > 0 && (
            <Cell title={`Average read; worst ${formatMicros(stats.readMaxUs)}, jitter ${formatMicros(stats.jitterUs)}`}>
              {stats.values} values in {stats.regions} {stats.regions === 1 ? "read" : "reads"},{" "}
              {formatMicros(stats.readAvgUs)}
            </Cell>
          )}
          {stats.skippedTicks > 0 && (
            <Cell title="Ticks skipped because reads ran past the next deadline" tone="warn">
              {stats.skippedTicks} skipped
            </Cell>
          )}
          {stats.failedRegions > 0 && (
            <Cell title={stats.lastError ?? undefined} tone="danger">
              {stats.failedRegions} failed reads
            </Cell>
          )}
          {link.carrier === "serial" ? (
            <Cell title="Framed link to the firmware over its USB port">USB link</Cell>
          ) : (
            <>
              <Cell tone={stats.core === "running" ? undefined : "warn"}>Core {coreText[stats.core].toLowerCase()}</Cell>
              {stats.log.state === "attached" && stats.log.blocking ? (
                <Cell
                  tone="warn"
                  title="This channel is in block-if-full mode (another tool may have set it). The firmware waits whenever the log buffer fills, so logging can stall control loops. Reset the board to restore the firmware's own mode."
                >
                  Log on RTT “{stats.log.channel}”, blocking
                </Cell>
              ) : (
                <Cell>
                  {stats.log.state === "attached"
                    ? `Log on RTT “${stats.log.channel}”`
                    : stats.log.state === "searching"
                      ? "Looking for RTT"
                      : "No RTT log"}
                </Cell>
              )}
            </>
          )}
        </>
      )}
    </footer>
  );
}
