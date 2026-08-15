import { useEffect, useState } from "react";
import { REPO_URL } from "../data";
import {
  detectOs,
  formatDate,
  formatSize,
  platformFor,
  useRelease,
  type Os,
} from "../useRelease";
import { useInView, useTypewriter } from "../hooks";

const OS_LABEL: Record<Os, string> = {
  windows: "Windows",
  linux: "Linux",
  macos: "macOS",
  unknown: "other",
};

export function Download({ reduced }: { reduced: boolean }) {
  const release = useRelease();
  const [os, setOs] = useState<Os>("unknown");
  const [copied, setCopied] = useState(false);
  const { ref, seen } = useInView<HTMLDivElement>();

  // Detection runs after mount rather than during render: it reads `navigator`,
  // which does not exist if this is ever prerendered.
  useEffect(() => setOs(detectOs()), []);

  // "copied" is a confirmation, not a state. It goes back to being a button.
  useEffect(() => {
    if (!copied) return;
    const id = window.setTimeout(() => setCopied(false), 2000);
    return () => window.clearTimeout(id);
  }, [copied]);

  const platform = platformFor(os, release);
  const typed = useTypewriter(platform.command, reduced, seen);

  const version = release.version;
  const date = formatDate(release.published);

  return (
    <div className="download" ref={ref}>
      {platform.primary ? (
        <a className="dl-primary" href={platform.primary.url}>
          <span>{platform.primary.label}</span>
          <span className="arrow">↓</span>
        </a>
      ) : (
        <a className="dl-primary" href={`${REPO_URL}#install`}>
          <span>
            {os === "macos" ? "Build from source" : "Every artifact on GitHub"}
          </span>
          <span className="arrow">→</span>
        </a>
      )}

      <div className="dl-meta">
        v{version}
        {date ? ` · ${date}` : ""}
        {platform.primary ? ` · ${platform.primary.detail}` : ""}
        {platform.primary?.size ? ` · ${formatSize(platform.primary.size)}` : ""}
      </div>

      <div className="prose" style={{ fontSize: 13, maxWidth: "46ch" }}>
        {platform.note}
      </div>

      {platform.alternatives.length > 0 && (
        <div className="dl-alts">
          {platform.alternatives.map((a) => (
            <a className="dl-alt" key={a.url} href={a.url} title={`${a.detail} · ${formatSize(a.size)}`}>
              {a.label}
            </a>
          ))}
        </div>
      )}

      <div className="install-box">
        <span className="sigil">{platform.prompt}</span>
        <span className="cmd">{typed}</span>
        <span className="caret blink" style={{ width: 8, height: 15 }} />
        <button
          className="copy"
          onClick={() => {
            navigator.clipboard?.writeText(platform.command).then(
              () => setCopied(true),
              // No clipboard permission, or an insecure origin. The command is
              // on screen and selectable either way.
              () => setCopied(false),
            );
          }}
        >
          {copied ? "copied" : "copy"}
        </button>
      </div>

      <div className="dl-alts">
        {(["windows", "linux", "macos"] as Os[])
          .filter((o) => o !== os)
          .map((o) => (
            <button className="dl-alt" key={o} onClick={() => setOs(o)}>
              {OS_LABEL[o]} instead
            </button>
          ))}
        <a className="dl-alt" href={`${REPO_URL}/releases/latest`}>
          all files
        </a>
      </div>
    </div>
  );
}
