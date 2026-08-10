import { QRCodeSVG } from "qrcode.react";
import type { AuthStatus } from "./types";

const AUTH_EXPIRY_MS = 10 * 60 * 1000;

type AuthPanelProps = {
  auth: AuthStatus;
  authUrl: string | null;
  authStartedAt: number | null;
  busy: boolean;
  compact?: boolean;
  onAuth: () => void;
  onCopy: () => void;
  onOpen: () => void;
  onRestart: () => void;
  onSignOut: () => void;
};

export function authExpired(authStartedAt: number | null, now = Date.now()): boolean {
  if (!authStartedAt) return false;
  return now - authStartedAt >= AUTH_EXPIRY_MS;
}

export function AuthPanel({
  auth,
  authUrl,
  authStartedAt,
  busy,
  compact = false,
  onAuth,
  onCopy,
  onOpen,
  onRestart,
  onSignOut,
}: AuthPanelProps) {
  const expired = Boolean(authUrl) && authExpired(authStartedAt);
  const waiting = Boolean(authUrl) && !auth.authenticated;

  async function copyUrl() {
    if (!authUrl) return;
    onCopy();
    try {
      await navigator.clipboard.writeText(authUrl);
    } catch {
      // Clipboard may be unavailable in restricted contexts; URL remains visible.
    }
  }

  if (auth.authenticated) {
    return (
      <section className={`panel auth-banner connected ${compact ? "compact" : ""}`}>
        <div className="auth-mark" aria-hidden="true">
          <KeyMark />
        </div>
        <div>
          <p className="eyebrow">Ready to publish and tag</p>
          <h2>Pubky authorization active</h2>
          <p>
            Scoped grant covers releases and tag claims. Root keys stay on your
            signing device. Session ends when you quit the app.
          </p>
          {auth.user && <code>{auth.user}</code>}
        </div>
        <div className="auth-actions">
          <button className="secondary" type="button" disabled={busy} onClick={onSignOut}>
            Sign out
          </button>
        </div>
      </section>
    );
  }

  return (
    <section className={`panel auth-banner ${waiting ? "waiting" : ""} ${compact ? "compact" : ""}`}>
      <div className="auth-mark" aria-hidden="true">
        <KeyMark />
      </div>
      <div>
        <p className="eyebrow">Publisher access required</p>
        <h2>{waiting ? "Approve in Pubky Ring" : "Connect your Pubky"}</h2>
        <p>
          Grant covers <code>/pub/pubky.swarm/v1/releases/</code> and{" "}
          <code>/pub/pubky.swarm/v1/tag-claims/</code>. Scan the QR or copy the
          deep link into Ring.
        </p>
      </div>
      {!waiting && (
        <div className="auth-actions">
          <button className="primary" type="button" disabled={busy} onClick={onAuth}>
            Authorize with Pubky
          </button>
        </div>
      )}
      {waiting && authUrl && (
        <div className="auth-qr-panel" data-testid="auth-qr-panel">
          <div className="auth-qr" aria-label="Authorization QR code">
            <QRCodeSVG value={authUrl} size={compact ? 148 : 196} level="M" includeMargin />
          </div>
          <label className="auth-url-field">
            Authorization URL
            <textarea
              readOnly
              value={authUrl}
              rows={3}
              spellCheck={false}
              aria-label="Authorization URL"
            />
          </label>
          <div className="auth-actions">
            <button className="primary" type="button" onClick={() => void copyUrl()} data-testid="copy-auth-url">
              Copy
            </button>
            <button className="secondary" type="button" onClick={onOpen} data-testid="open-auth-url">
              Open
            </button>
            <button className="secondary" type="button" disabled={busy} onClick={onRestart}>
              Start over
            </button>
          </div>
          <p className="auth-wait" role="status">
            {expired
              ? "This grant request may have expired. Start over and approve again in Ring."
              : "Waiting for Ring approval…"}
          </p>
        </div>
      )}
    </section>
  );
}

function KeyMark() {
  return (
    <svg viewBox="0 0 24 24" width="22" height="22" fill="none" aria-hidden="true">
      <path
        d="M8.5 14.5a3.5 3.5 0 1 1 0-7 3.5 3.5 0 0 1 0 7Zm0 0 7.5 7.5M14 16.5l2.5 2.5M16.5 14l2.5 2.5"
        stroke="currentColor"
        strokeWidth="1.7"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
