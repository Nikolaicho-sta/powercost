import type { ReactNode } from "react";
import { dot } from "../tariffModel";
import { InfoTip } from "./InfoTip";

export function Field({ label, help, children, hint }: { label: string; help?: string; hint?: string; children: ReactNode }) {
  return (
    <label className="field">
      <span className="field-label">
        {label}
        {help && <InfoTip text={help} />}
      </span>
      {children}
      {hint && <span className="note">{hint}</span>}
    </label>
  );
}

export function Check({ label, checked, onChange, help, disabled }: { label: string; checked: boolean; onChange: (v: boolean) => void; help?: string; disabled?: boolean }) {
  return (
    <label className="check">
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>{label}</span>
      {help && <InfoTip text={help} />}
    </label>
  );
}

/** Text input for numbers that must stay exact strings (Rust Decimal values). */
export function Dec({ value, onChange, unit, width, ariaLabel }: { value: string; onChange: (v: string) => void; unit?: string; width?: number; ariaLabel?: string }) {
  return (
    <span className="inline-input">
      <input inputMode="decimal" value={value} aria-label={ariaLabel} style={width ? { width } : undefined} onChange={(e) => onChange(dot(e.target.value))} />
      {unit && <span className="muted">{unit}</span>}
    </span>
  );
}

export function IntInput({ value, onChange, unit, min, max, ariaLabel }: { value: number; onChange: (v: number) => void; unit?: string; min?: number; max?: number; ariaLabel?: string }) {
  return (
    <span className="inline-input">
      <input type="number" value={Number.isFinite(value) ? value : ""} min={min} max={max} aria-label={ariaLabel} onChange={(e) => onChange(e.target.value === "" ? NaN : Math.trunc(Number(e.target.value)))} />
      {unit && <span className="muted">{unit}</span>}
    </span>
  );
}

export function Notice({ kind, children }: { kind: "error" | "ok" | "info"; children: ReactNode }) {
  return (
    <div className={`notice ${kind}`} role={kind === "error" ? "alert" : "status"}>
      {children}
    </div>
  );
}

export function Badge({ children, tone }: { children: ReactNode; tone?: "good" | "warn" | "muted" }) {
  return <span className={`pill ${tone ?? "muted"}`}>{children}</span>;
}

export function verificationBadge(v: string | null | undefined) {
  switch (v) {
    case "verified":
      return <Badge tone="good">verified</Badge>;
    case "partial":
      return <Badge tone="warn">partly verified</Badge>;
    case "unverified":
      return <Badge tone="warn">unverified</Badge>;
    case "example":
      return <Badge>example</Badge>;
    default:
      return <Badge>no source info</Badge>;
  }
}

export function Modal({ title, children, onClose, wide }: { title: string; children: ReactNode; onClose?: () => void; wide?: boolean }) {
  return (
    <div className="modal-back" role="dialog" aria-modal="true" aria-label={title}>
      <div className={`modal${wide ? " wide" : ""}`}>
        <div className="modal-head">
          <h2>{title}</h2>
          {onClose && (
            <button type="button" className="icon-btn" onClick={onClose} aria-label="Close">
              ✕
            </button>
          )}
        </div>
        {children}
      </div>
    </div>
  );
}
