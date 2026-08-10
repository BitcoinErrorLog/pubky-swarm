import type { SubjectTagClaim } from "./types";

export function shortIssuer(value: string) {
  if (value.length <= 12) return value;
  return `${value.slice(0, 6)}…${value.slice(-4)}`;
}

export function PublisherTags({ tags }: { tags: string[] }) {
  if (tags.length === 0) return null;
  return (
    <div className="tags publisher-tags" aria-label="Publisher tags">
      {tags.map((tag) => (
        <span key={`publisher:${tag}`} title="Publisher metadata">
          #{tag}
        </span>
      ))}
    </div>
  );
}

export function ClaimChips({
  claims,
  emptyLabel,
}: {
  claims: SubjectTagClaim[];
  emptyLabel?: string;
}) {
  if (claims.length === 0) {
    return emptyLabel ? <p className="claim-empty">{emptyLabel}</p> : null;
  }
  return (
    <div className="tags claim-tags" aria-label="Tag claims">
      {claims.map((claim) => (
        <span
          className="claimed"
          key={`${claim.issuer}:${claim.tag}:${claim.revision}`}
          title={`Claim by ${claim.issuer}`}
        >
          #{claim.tag} · {shortIssuer(claim.issuer)}
        </span>
      ))}
    </div>
  );
}

export function TagPublishRow({
  draft,
  authenticated,
  busy,
  disabled,
  onDraftChange,
  onPublish,
  onNeedAuth,
}: {
  draft: string;
  authenticated: boolean;
  busy: boolean;
  disabled?: boolean;
  onDraftChange: (value: string) => void;
  onPublish: () => void;
  onNeedAuth: () => void;
}) {
  return (
    <div className="tag-publisher">
      <input
        value={draft}
        onChange={(event) => onDraftChange(event.target.value)}
        placeholder="public-domain, research"
        maxLength={512}
        aria-label="Public tag claims"
        disabled={disabled}
      />
      {authenticated ? (
        <button
          className="secondary compact"
          type="button"
          disabled={busy || disabled || !draft.trim()}
          onClick={onPublish}
        >
          Publish tags
        </button>
      ) : (
        <button className="secondary compact" type="button" onClick={onNeedAuth}>
          Connect to tag
        </button>
      )}
    </div>
  );
}
