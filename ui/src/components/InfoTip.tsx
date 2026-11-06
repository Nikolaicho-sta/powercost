import { useId } from "react";

/** Info tooltip: opens on hover and keyboard focus. Every estimate gets one (CLAUDE.md). */
export function InfoTip({ text, right }: { text: string; right?: boolean }) {
  const id = useId();
  return (
    <span className={`tip${right ? " right" : ""}`}>
      <button type="button" className="tip-btn" aria-describedby={id} aria-label="How is this estimated?">
        i
      </button>
      <span role="tooltip" id={id} className="tip-body">
        {text}
      </span>
    </span>
  );
}
