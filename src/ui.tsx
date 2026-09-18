import { ReactNode } from "react";

export const button = "rounded-sm border border-rule bg-panel px-2.5 py-0.5 hover:bg-sunken disabled:opacity-50 disabled:hover:bg-panel";
export const primaryButton =
  "rounded-sm border border-accent bg-accent-wash px-2.5 py-0.5 font-medium hover:brightness-95 disabled:opacity-50";
export const ghostButton =
  "rounded-sm px-1.5 py-0.5 text-muted hover:bg-sunken hover:text-ink disabled:opacity-50 disabled:hover:bg-transparent";
export const field = "rounded-sm border border-rule bg-surface px-1.5 py-0.5 disabled:opacity-60";

interface Option<T> {
  value: T;
  label: ReactNode;
  title?: string;
  disabled?: boolean;
}

/** A row of mutually exclusive buttons; `value` null leaves every one released */
export function Segmented<T extends string | number>({
  label,
  options,
  value,
  onChange,
  disabled,
}: {
  label: string;
  options: Option<T>[];
  value: T | null;
  onChange: (value: T) => void;
  disabled?: boolean;
}) {
  return (
    <span role="radiogroup" aria-label={label} className="inline-flex rounded-sm border border-rule bg-panel p-px">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={o.value === value}
          disabled={disabled || o.disabled}
          title={o.title}
          onClick={() => onChange(o.value)}
          className={`rounded-[1px] px-2 py-px ${
            o.value === value
              ? "bg-surface text-ink shadow-[0_0_0_1px_var(--rule)]"
              : "text-muted enabled:hover:text-ink disabled:opacity-45"
          }`}
        >
          {o.label}
        </button>
      ))}
    </span>
  );
}

export function TabStrip({ children, tools }: { children: ReactNode; tools?: ReactNode }) {
  return (
    <div role="tablist" className="flex shrink-0 items-end gap-0.5 border-b border-rule bg-panel px-2 pt-1.5">
      {children}
      {tools && <div className="ml-auto flex min-w-0 items-center gap-1 pb-1 text-[12px]">{tools}</div>}
    </div>
  );
}

export function Tab({
  selected,
  onSelect,
  count,
  disabled,
  title,
  children,
}: {
  selected: boolean;
  onSelect: () => void;
  count?: number | null;
  disabled?: boolean;
  title?: string;
  children: ReactNode;
}) {
  return (
    <button
      role="tab"
      aria-selected={selected}
      disabled={disabled}
      title={title}
      onClick={onSelect}
      className={`-mb-px rounded-t-sm border border-b-0 px-3 py-0.5 disabled:opacity-50 ${
        selected ? "border-rule bg-surface text-ink" : "border-transparent text-muted enabled:hover:text-ink"
      }`}
    >
      {children}
      {count !== undefined && count !== null && <span className="ml-1 text-faint">{count}</span>}
    </button>
  );
}
