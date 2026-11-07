import { useState } from "react";
import { api, errorLines, type UpdateStatus } from "../api";
import { Notice } from "./ui";

/**
 * Two-step start: the first click asks, the second one runs the update. The update script closes
 * PowerCost, so the user is told that before anything happens.
 */
export function useStartUpdate() {
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const confirm = async () => {
    setAsking(false);
    setBusy(true);
    setError(null);
    try {
      await api.startUpdate();
    } catch (e) {
      setError(errorLines(e).join(" "));
      setBusy(false);
    }
  };
  return { asking, busy, error, ask: () => setAsking(true), cancel: () => setAsking(false), confirm };
}

export function UpdateButtons({ u, canUpdate, extra }: { u: ReturnType<typeof useStartUpdate>; canUpdate: boolean; extra?: React.ReactNode }) {
  return (
    <>
      {u.error && <Notice kind="error">{u.error}</Notice>}
      {u.asking && (
        <Notice kind="info">
          PowerCost will close, rebuild (a few minutes; a window shows the progress) and start again. Your data is kept; tracking pauses while it is closed.
        </Notice>
      )}
      <div className="controls">
        {u.asking ? (
          <>
            <button type="button" className="btn primary" onClick={u.confirm}>Yes, update and restart</button>
            <button type="button" className="btn" onClick={u.cancel}>Cancel</button>
          </>
        ) : (
          <>
            <button type="button" className="btn primary" disabled={u.busy || !canUpdate} onClick={u.ask}>
              {u.busy ? "Starting the update..." : "Update now"}
            </button>
            {extra}
          </>
        )}
      </div>
    </>
  );
}

/** The "Updates" card in Settings: status, "Check for updates" and the one-click "Update now". */
export function UpdateBox({ status, onStatus }: { status: UpdateStatus; onStatus: (s: UpdateStatus) => void }) {
  const u = useStartUpdate();
  const [checking, setChecking] = useState(false);

  const check = async () => {
    setChecking(true);
    try {
      onStatus(await api.checkUpdate());
    } finally {
      setChecking(false);
    }
  };

  return (
    <div>
      {status.state === "available" && (
        <Notice kind="info">
          <strong>A newer version is ready:</strong> {status.subject || status.hash.slice(0, 7)} <span className="muted">({status.hash.slice(0, 7)})</span>
        </Notice>
      )}
      {status.state === "up_to_date" && <p className="note" style={{ marginTop: 0 }}>You have the newest version from the project folder.</p>}
      {status.state === "unavailable" && <p className="note" style={{ marginTop: 0 }}>Updates are not available: {status.reason}.</p>}
      {status.state === "unknown" && <p className="note" style={{ marginTop: 0 }}>Not checked yet.</p>}
      <UpdateButtons
        u={u}
        canUpdate={status.state === "available"}
        extra={
          <button type="button" className="btn" disabled={checking || u.busy} onClick={check}>
            {checking ? "Checking..." : "Check for updates"}
          </button>
        }
      />
      <p className="note">
        The check is local: it compares this app with the newest version in the project folder it was built from. Nothing is downloaded and nothing goes online.
      </p>
    </div>
  );
}

/** Banner shown on every page when an update is ready. */
export function UpdateBanner({ status }: { status: UpdateStatus }) {
  const u = useStartUpdate();
  if (status.state !== "available") return null;
  return (
    <div className="banner" role="status">
      <strong>Update available:</strong> {status.subject || status.hash.slice(0, 7)}
      <UpdateButtons u={u} canUpdate />
    </div>
  );
}
